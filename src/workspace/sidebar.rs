//! Non-blocking domain model for a workspace file sidebar.
//!
//! Filesystem and Git operations run on one bounded background worker. Callers
//! drive completion by calling [`Sidebar::tick`].

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

/// Default maximum number of children loaded from one directory.
pub const DEFAULT_ENTRY_CAP: usize = 10_000;

#[derive(Debug, Clone)]
pub struct SidebarConfig {
    pub entry_cap: usize,
    pub channel_capacity: usize,
    pub git_refresh_interval: Duration,
}

impl Default for SidebarConfig {
    fn default() -> Self {
        Self {
            entry_cap: DEFAULT_ENTRY_CAP,
            channel_capacity: 8,
            git_refresh_interval: Duration::from_secs(3),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    File,
    SymlinkDirectory,
    Symlink,
    Other,
    /// A tracked path reported deleted by Git but absent from the filesystem.
    Deleted,
}

impl EntryKind {
    pub fn is_directory(self) -> bool {
        matches!(self, Self::Directory | Self::SymlinkDirectory)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GitStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Conflict,
    Untracked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GitDecoration {
    pub status: Option<GitStatus>,
    pub dirty_descendant: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitChange {
    /// Workspace-relative path, represented without UTF-8 conversion on Unix.
    pub path: PathBuf,
    pub original_path: Option<PathBuf>,
    pub status: GitStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarRow {
    pub path: PathBuf,
    pub name: OsString,
    pub depth: usize,
    pub kind: EntryKind,
    pub expanded: bool,
    pub loading: bool,
    pub selected: bool,
    pub error: Option<String>,
    pub git: GitDecoration,
}

impl SidebarRow {
    /// Display-safe name with replacement characters only for invalid Unicode.
    /// The lossless [`PathBuf`] identity remains available in [`Self::path`].
    pub fn display_name(&self) -> std::borrow::Cow<'_, str> {
        self.name.to_string_lossy()
    }
}

#[derive(Debug)]
struct Node {
    name: OsString,
    kind: EntryKind,
    expanded: bool,
    loading: bool,
    loaded: bool,
    error: Option<String>,
    children: Vec<PathBuf>,
    canonical_dir: Option<PathBuf>,
    synthetic_deleted: bool,
    git: GitDecoration,
}

impl Node {
    fn row(&self, path: PathBuf, depth: usize, selected: bool) -> SidebarRow {
        SidebarRow {
            path,
            name: self.name.clone(),
            depth,
            kind: self.kind,
            expanded: self.expanded,
            loading: self.loading,
            selected,
            error: self.error.clone(),
            git: self.git,
        }
    }
}

enum Job {
    Load {
        id: u64,
        path: PathBuf,
        root: PathBuf,
        ancestor_targets: Vec<PathBuf>,
        cap: usize,
    },
    Git {
        id: u64,
        root: PathBuf,
    },
}

enum Response {
    Load {
        id: u64,
        path: PathBuf,
        result: Result<LoadedDirectory, String>,
    },
    Git {
        id: u64,
        result: Result<Vec<GitChange>, String>,
    },
}

struct LoadedDirectory {
    canonical: PathBuf,
    entries: Vec<LoadedEntry>,
    truncated: bool,
}

struct LoadedEntry {
    path: PathBuf,
    name: OsString,
    kind: EntryKind,
    canonical_dir: Option<PathBuf>,
    error: Option<String>,
}

/// Stateful, non-blocking sidebar model.
///
/// `request_*`, `click_visible_row`, and `tick` never perform filesystem or
/// process I/O on the calling thread. A bounded channel applies backpressure;
/// request methods return `false` when the worker queue is full.
pub struct Sidebar {
    root: PathBuf,
    config: SidebarConfig,
    nodes: HashMap<PathBuf, Node>,
    selected: Option<PathBuf>,
    scroll: usize,
    jobs: SyncSender<Job>,
    responses: Receiver<Response>,
    next_id: u64,
    pending_loads: HashMap<PathBuf, u64>,
    pending_git: Option<u64>,
    last_git_request: Instant,
    git_error: Option<String>,
}

impl Sidebar {
    pub fn new(root: impl Into<PathBuf>) -> io::Result<Self> {
        Self::with_config(root, SidebarConfig::default())
    }

    pub fn with_config(root: impl Into<PathBuf>, config: SidebarConfig) -> io::Result<Self> {
        if config.entry_cap == 0 || config.channel_capacity == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sidebar entry cap and channel capacity must be non-zero",
            ));
        }
        let root = root.into();
        let name = root
            .file_name()
            .map(OsStr::to_os_string)
            .unwrap_or_else(|| root.as_os_str().to_os_string());
        let mut nodes = HashMap::new();
        nodes.insert(
            root.clone(),
            Node {
                name,
                kind: EntryKind::Directory,
                expanded: false,
                loading: false,
                loaded: false,
                error: None,
                children: Vec::new(),
                canonical_dir: Some(root.clone()),
                synthetic_deleted: false,
                git: GitDecoration::default(),
            },
        );

        let (job_tx, job_rx) = mpsc::sync_channel(config.channel_capacity);
        let (response_tx, response_rx) = mpsc::sync_channel(config.channel_capacity);
        thread::Builder::new()
            .name("sidebar-worker".into())
            .spawn(move || worker(job_rx, response_tx))?;

        Ok(Self {
            root,
            config,
            nodes,
            selected: None,
            scroll: 0,
            jobs: job_tx,
            responses: response_rx,
            next_id: 1,
            pending_loads: HashMap::new(),
            pending_git: None,
            last_git_request: Instant::now(),
            git_error: None,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Expands and loads the root and requests Git state.
    pub fn request_initial(&mut self) {
        let root = self.root.clone();
        self.request_expand(&root);
        self.request_git_refresh();
    }

    /// Expands `path`, queueing a load if necessary. Returns whether a load was
    /// queued (an already-loaded directory is still expanded and returns false).
    pub fn request_expand(&mut self, path: &Path) -> bool {
        let Some(node) = self.nodes.get(path) else {
            return false;
        };
        if !node.kind.is_directory() {
            return false;
        }
        let needs_load = !node.loaded;
        if !needs_load {
            if let Some(node) = self.nodes.get_mut(path) {
                node.expanded = true;
            }
            return false;
        }

        let id = self.take_id();
        let ancestor_targets = self.ancestor_targets(path);
        let job = Job::Load {
            id,
            path: path.to_path_buf(),
            root: self.root.clone(),
            ancestor_targets,
            cap: self.config.entry_cap,
        };
        match self.jobs.try_send(job) {
            Ok(()) => {
                let node = self.nodes.get_mut(path).expect("node checked above");
                node.expanded = true;
                node.loading = true;
                node.error = None;
                self.pending_loads.insert(path.to_path_buf(), id);
                true
            }
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Invalidates and reloads an expanded directory. Older in-flight replies
    /// are rejected by request id.
    pub fn request_reload(&mut self, path: &Path) -> bool {
        if let Some(node) = self.nodes.get_mut(path) {
            if !node.kind.is_directory() {
                return false;
            }
            node.loaded = false;
        } else {
            return false;
        }
        self.request_expand(path)
    }

    pub fn collapse(&mut self, path: &Path) {
        if let Some(node) = self.nodes.get_mut(path) {
            node.expanded = false;
            node.loading = false;
        }
        self.pending_loads.remove(path);
    }

    pub fn request_git_refresh(&mut self) -> bool {
        if self.pending_git.is_some() {
            return false;
        }
        let id = self.take_id();
        match self.jobs.try_send(Job::Git {
            id,
            root: self.root.clone(),
        }) {
            Ok(()) => {
                self.pending_git = Some(id);
                self.last_git_request = Instant::now();
                true
            }
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Applies all currently available worker replies and schedules periodic Git
    /// refreshes. Returns `true` if model state changed.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        loop {
            match self.responses.try_recv() {
                Ok(response) => {
                    changed |= self.apply_response(response);
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        if self.pending_git.is_none()
            && self.last_git_request.elapsed() >= self.config.git_refresh_interval
        {
            self.request_git_refresh();
        }
        changed
    }

    /// Rows after expansion and scroll are applied, limited to `viewport_rows`.
    pub fn visible_rows(&self, viewport_rows: usize) -> Vec<SidebarRow> {
        self.flatten_rows()
            .into_iter()
            .skip(self.scroll)
            .take(viewport_rows)
            .collect()
    }

    /// All expanded rows, before scroll/viewport clipping.
    pub fn all_visible_rows(&self) -> Vec<SidebarRow> {
        self.flatten_rows()
    }

    /// Selects a row relative to the current viewport and toggles directories.
    pub fn click_visible_row(&mut self, row: usize, viewport_rows: usize) -> Option<PathBuf> {
        if row >= viewport_rows {
            return None;
        }
        let path = self
            .flatten_rows()
            .get(self.scroll.saturating_add(row))?
            .path
            .clone();
        self.selected = Some(path.clone());
        let directory = self
            .nodes
            .get(&path)
            .is_some_and(|node| node.kind.is_directory());
        if directory {
            if self.nodes.get(&path).is_some_and(|node| node.expanded) {
                self.collapse(&path);
            } else {
                self.request_expand(&path);
            }
        }
        Some(path)
    }

    pub fn scroll(&mut self, delta: isize, viewport_rows: usize) {
        let total = self.flatten_rows().len();
        let maximum = total.saturating_sub(viewport_rows);
        self.scroll = self.scroll.saturating_add_signed(delta).min(maximum);
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll
    }

    pub fn selected_path(&self) -> Option<&Path> {
        self.selected.as_deref()
    }

    /// Last Git worker error. Non-Git workspaces and command failures degrade to
    /// an undecorated tree rather than making filesystem browsing unavailable.
    pub fn git_error(&self) -> Option<&str> {
        self.git_error.as_deref()
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    fn ancestor_targets(&self, path: &Path) -> Vec<PathBuf> {
        let mut result = Vec::new();
        let mut cursor = path.parent();
        while let Some(parent) = cursor {
            if let Some(target) = self
                .nodes
                .get(parent)
                .and_then(|node| node.canonical_dir.clone())
            {
                result.push(target);
            }
            if parent == self.root {
                break;
            }
            cursor = parent.parent();
        }
        result
    }

    fn apply_response(&mut self, response: Response) -> bool {
        match response {
            Response::Load { id, path, result } => {
                if self.pending_loads.get(&path).copied() != Some(id) {
                    return false;
                }
                self.pending_loads.remove(&path);
                let Some(parent) = self.nodes.get_mut(&path) else {
                    return false;
                };
                parent.loading = false;
                match result {
                    Err(error) => {
                        parent.error = Some(error);
                        parent.loaded = false;
                    }
                    Ok(loaded) => {
                        parent.loaded = true;
                        parent.canonical_dir = Some(loaded.canonical);
                        parent.error = loaded
                            .truncated
                            .then(|| format!("entry cap reached ({})", self.config.entry_cap));
                        let old_children = std::mem::take(&mut parent.children);
                        let new_paths: HashSet<_> = loaded
                            .entries
                            .iter()
                            .map(|entry| entry.path.clone())
                            .collect();
                        for old in old_children {
                            if !new_paths.contains(&old) {
                                self.remove_subtree(&old);
                            }
                        }
                        let mut children = Vec::with_capacity(loaded.entries.len());
                        for entry in loaded.entries {
                            children.push(entry.path.clone());
                            let old = self.nodes.remove(&entry.path);
                            self.nodes.insert(
                                entry.path,
                                Node {
                                    name: entry.name,
                                    kind: entry.kind,
                                    expanded: old.as_ref().is_some_and(|n| n.expanded),
                                    loading: false,
                                    loaded: old.as_ref().is_some_and(|n| n.loaded),
                                    error: entry.error,
                                    children: old.map_or_else(Vec::new, |n| n.children),
                                    canonical_dir: entry.canonical_dir,
                                    synthetic_deleted: false,
                                    git: GitDecoration::default(),
                                },
                            );
                        }
                        if let Some(parent) = self.nodes.get_mut(&path) {
                            parent.children = children;
                        }
                    }
                }
                true
            }
            Response::Git { id, result } => {
                if self.pending_git != Some(id) {
                    return false;
                }
                self.pending_git = None;
                match result {
                    Ok(changes) => {
                        self.git_error = None;
                        self.apply_git(changes);
                    }
                    Err(error) => {
                        self.git_error = Some(error);
                        self.clear_git();
                    }
                }
                true
            }
        }
    }

    fn remove_subtree(&mut self, path: &Path) {
        if let Some(node) = self.nodes.remove(path) {
            self.pending_loads.remove(path);
            for child in node.children {
                self.remove_subtree(&child);
            }
        }
    }

    fn clear_git(&mut self) {
        let synthetic: Vec<_> = self
            .nodes
            .iter()
            .filter(|(_, node)| node.synthetic_deleted)
            .map(|(path, _)| path.clone())
            .collect();
        for path in synthetic {
            if let Some(parent) = path.parent().and_then(|p| self.nodes.get_mut(p)) {
                parent.children.retain(|child| child != &path);
            }
            self.nodes.remove(&path);
        }
        for node in self.nodes.values_mut() {
            node.git = GitDecoration::default();
        }
    }

    fn apply_git(&mut self, changes: Vec<GitChange>) {
        self.clear_git();
        for change in changes {
            let Some(path) = safe_join(&self.root, &change.path) else {
                continue;
            };
            if change.status == GitStatus::Deleted && !self.nodes.contains_key(&path) {
                if let Some(parent_path) = path.parent() {
                    let visible_parent = self
                        .nodes
                        .get(parent_path)
                        .is_some_and(|node| node.loaded && node.expanded);
                    if visible_parent {
                        let name = path.file_name().unwrap_or_default().to_os_string();
                        self.nodes.insert(
                            path.clone(),
                            Node {
                                name,
                                kind: EntryKind::Deleted,
                                expanded: false,
                                loading: false,
                                loaded: true,
                                error: None,
                                children: Vec::new(),
                                canonical_dir: None,
                                synthetic_deleted: true,
                                git: GitDecoration {
                                    status: Some(GitStatus::Deleted),
                                    dirty_descendant: false,
                                },
                            },
                        );
                        let mut children = self
                            .nodes
                            .get_mut(parent_path)
                            .map(|parent| {
                                parent.children.push(path.clone());
                                std::mem::take(&mut parent.children)
                            })
                            .unwrap_or_default();
                        sort_child_paths(&mut children, &self.nodes);
                        if let Some(parent) = self.nodes.get_mut(parent_path) {
                            parent.children = children;
                        }
                    }
                }
            }
            if let Some(node) = self.nodes.get_mut(&path) {
                node.git.status = Some(merge_status(node.git.status, change.status));
            }
            let mut ancestor = path.parent();
            while let Some(parent) = ancestor {
                if !parent.starts_with(&self.root) {
                    break;
                }
                if let Some(node) = self.nodes.get_mut(parent) {
                    node.git.dirty_descendant = true;
                }
                if parent == self.root {
                    break;
                }
                ancestor = parent.parent();
            }
        }
    }

    fn flatten_rows(&self) -> Vec<SidebarRow> {
        let mut rows = Vec::new();
        self.flatten_node(&self.root, 0, &mut rows);
        rows
    }

    fn flatten_node(&self, path: &Path, depth: usize, rows: &mut Vec<SidebarRow>) {
        let Some(node) = self.nodes.get(path) else {
            return;
        };
        rows.push(node.row(
            path.to_path_buf(),
            depth,
            self.selected.as_deref() == Some(path),
        ));
        if node.expanded {
            for child in &node.children {
                self.flatten_node(child, depth + 1, rows);
            }
        }
    }
}

fn worker(jobs: Receiver<Job>, responses: SyncSender<Response>) {
    while let Ok(job) = jobs.recv() {
        let response = match job {
            Job::Load {
                id,
                path,
                root,
                ancestor_targets,
                cap,
            } => Response::Load {
                id,
                path: path.clone(),
                result: load_directory(&root, &path, &ancestor_targets, cap),
            },
            Job::Git { id, root } => Response::Git {
                id,
                result: load_git(&root),
            },
        };
        if responses.send(response).is_err() {
            break;
        }
    }
}

fn load_directory(
    root: &Path,
    path: &Path,
    ancestor_targets: &[PathBuf],
    cap: usize,
) -> Result<LoadedDirectory, String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("failed to resolve {}: {error}", path.display()))?;
    if !canonical.starts_with(root) {
        return Err("symlink target escapes workspace".into());
    }
    if ancestor_targets
        .iter()
        .any(|ancestor| ancestor == &canonical)
    {
        return Err("symlink cycle through an ancestor".into());
    }

    let read = fs::read_dir(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let mut entries = Vec::new();
    for item in read {
        let item = item.map_err(|error| format!("failed to read directory entry: {error}"))?;
        let name = item.file_name();
        if name == OsStr::new(".git") {
            continue;
        }
        let entry_path = item.path();
        let file_type = item
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", entry_path.display()))?;
        let (kind, canonical_dir, error) = if file_type.is_symlink() {
            match entry_path.canonicalize() {
                Err(error) => (EntryKind::Symlink, None, Some(error.to_string())),
                Ok(target) if !target.starts_with(root) => (
                    EntryKind::Symlink,
                    None,
                    Some("symlink target escapes workspace".into()),
                ),
                Ok(target) => match fs::metadata(&entry_path) {
                    Ok(metadata) if metadata.is_dir() => {
                        let cycle = target == canonical
                            || ancestor_targets.iter().any(|ancestor| ancestor == &target);
                        if cycle {
                            (
                                EntryKind::Symlink,
                                None,
                                Some("symlink cycle through an ancestor".into()),
                            )
                        } else {
                            (EntryKind::SymlinkDirectory, Some(target), None)
                        }
                    }
                    Ok(_) => (EntryKind::Symlink, None, None),
                    Err(error) => (EntryKind::Symlink, None, Some(error.to_string())),
                },
            }
        } else if file_type.is_dir() {
            (EntryKind::Directory, entry_path.canonicalize().ok(), None)
        } else if file_type.is_file() {
            (EntryKind::File, None, None)
        } else {
            (EntryKind::Other, None, None)
        };
        entries.push(LoadedEntry {
            path: entry_path,
            name,
            kind,
            canonical_dir,
            error,
        });
    }
    entries.sort_by(|left, right| {
        let left_group = usize::from(!left.kind.is_directory());
        let right_group = usize::from(!right.kind.is_directory());
        left_group
            .cmp(&right_group)
            .then_with(|| left.name.cmp(&right.name))
    });
    let truncated = entries.len() > cap;
    entries.truncate(cap);
    Ok(LoadedDirectory {
        canonical,
        entries,
        truncated,
    })
}

fn load_git(root: &Path) -> Result<Vec<GitChange>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain=v2", "-z", "--untracked-files=all"])
        .output()
        .map_err(|error| format!("failed to run git status: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(if stderr.trim().is_empty() {
            format!("git status exited with {}", output.status)
        } else {
            stderr.trim().to_owned()
        });
    }
    parse_git_porcelain_v2(&output.stdout)
}

/// Parses `git status --porcelain=v2 -z` output.
///
/// Rename/copy records consume the second NUL-delimited original-path field.
pub fn parse_git_porcelain_v2(bytes: &[u8]) -> Result<Vec<GitChange>, String> {
    let records: Vec<&[u8]> = bytes.split(|byte| *byte == 0).collect();
    let mut changes = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        index += 1;
        if record.is_empty() || record.starts_with(b"# ") || record.starts_with(b"! ") {
            continue;
        }
        match record[0] {
            b'1' => {
                let fields = split_n_fields(record, 9)?;
                changes.push(GitChange {
                    status: status_from_xy(fields[1])?,
                    path: bytes_to_path(fields[8]),
                    original_path: None,
                });
            }
            b'2' => {
                let fields = split_n_fields(record, 10)?;
                let original = records
                    .get(index)
                    .ok_or_else(|| "rename record is missing original path".to_owned())?;
                index += 1;
                changes.push(GitChange {
                    status: status_from_xy(fields[1])?,
                    path: bytes_to_path(fields[9]),
                    original_path: Some(bytes_to_path(original)),
                });
            }
            b'u' => {
                let fields = split_n_fields(record, 11)?;
                changes.push(GitChange {
                    status: GitStatus::Conflict,
                    path: bytes_to_path(fields[10]),
                    original_path: None,
                });
            }
            b'?' => {
                let path = record
                    .strip_prefix(b"? ")
                    .ok_or_else(|| "malformed untracked record".to_owned())?;
                changes.push(GitChange {
                    status: GitStatus::Untracked,
                    path: bytes_to_path(path),
                    original_path: None,
                });
            }
            other => return Err(format!("unsupported porcelain v2 record type: {other}")),
        }
    }
    Ok(changes)
}

fn split_n_fields(record: &[u8], count: usize) -> Result<Vec<&[u8]>, String> {
    let fields: Vec<_> = record.splitn(count, |byte| *byte == b' ').collect();
    if fields.len() != count || fields.iter().any(|field| field.is_empty()) {
        Err("malformed porcelain v2 record".into())
    } else {
        Ok(fields)
    }
}

fn status_from_xy(xy: &[u8]) -> Result<GitStatus, String> {
    if xy.len() != 2 {
        return Err("malformed porcelain v2 XY status".into());
    }
    if xy.contains(&b'U') || matches!(xy, b"AA" | b"DD") {
        Ok(GitStatus::Conflict)
    } else if xy.contains(&b'D') {
        Ok(GitStatus::Deleted)
    } else if xy.contains(&b'R') || xy.contains(&b'C') {
        Ok(GitStatus::Renamed)
    } else if xy.contains(&b'A') {
        Ok(GitStatus::Added)
    } else if xy.contains(&b'M') || xy.contains(&b'T') {
        Ok(GitStatus::Modified)
    } else {
        Err("porcelain record has no recognized status".into())
    }
}

fn merge_status(existing: Option<GitStatus>, incoming: GitStatus) -> GitStatus {
    fn priority(status: GitStatus) -> u8 {
        match status {
            GitStatus::Conflict => 6,
            GitStatus::Deleted => 5,
            GitStatus::Renamed => 4,
            GitStatus::Added => 3,
            GitStatus::Modified => 2,
            GitStatus::Untracked => 1,
        }
    }
    existing
        .filter(|status| priority(*status) >= priority(incoming))
        .unwrap_or(incoming)
}

fn safe_join(root: &Path, relative: &Path) -> Option<PathBuf> {
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    Some(root.join(relative))
}

fn sort_child_paths(children: &mut [PathBuf], nodes: &HashMap<PathBuf, Node>) {
    children.sort_by(|left, right| {
        let left_node = nodes.get(left);
        let right_node = nodes.get(right);
        let left_group = usize::from(!left_node.is_some_and(|node| node.kind.is_directory()));
        let right_group = usize::from(!right_node.is_some_and(|node| node.kind.is_directory()));
        left_group.cmp(&right_group).then_with(|| {
            left_node
                .map(|node| &node.name)
                .cmp(&right_node.map(|node| &node.name))
        })
    });
}

#[cfg(unix)]
fn bytes_to_path(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(OsString::from_vec(bytes.to_vec()))
}

#[cfg(not(unix))]
fn bytes_to_path(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("sidebar-{label}-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    }

    fn wait(sidebar: &mut Sidebar) {
        for _ in 0..200 {
            if sidebar.tick() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("worker did not reply");
    }

    #[test]
    fn root_is_lazy_and_loading_sorts_shows_hidden_excludes_git_and_caps() {
        let root = temp_dir("tree");
        fs::create_dir(root.join("z-dir")).unwrap();
        fs::write(root.join("b"), b"").unwrap();
        fs::write(root.join("c"), b"").unwrap();
        fs::write(root.join(".hidden"), b"").unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        let config = SidebarConfig {
            entry_cap: 3,
            ..SidebarConfig::default()
        };
        let mut sidebar = Sidebar::with_config(&root, config).unwrap();
        assert_eq!(sidebar.all_visible_rows().len(), 1);
        assert!(sidebar.request_expand(&root));
        wait(&mut sidebar);
        let rows = sidebar.all_visible_rows();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1].name, OsStr::new("z-dir"));
        assert!(rows.iter().any(|row| row.name == OsStr::new(".hidden")));
        assert!(!rows.iter().any(|row| row.name == OsStr::new(".git")));
        assert!(rows[0].error.as_deref().unwrap().contains("entry cap"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn porcelain_v2_parses_all_relevant_records_and_two_nul_rename() {
        let input = b"1 M. N... 100644 100644 100644 abc abc src/a file\0\
2 R. N... 100644 100644 100644 abc abc R100 new name\0old name\0\
u UU N... 100644 100644 100644 100644 a b c conflict\0\
? untracked\0\
1 .D N... 100644 100644 000000 abc abc deleted\0";
        let parsed = parse_git_porcelain_v2(input).unwrap();
        assert_eq!(parsed.len(), 5);
        assert_eq!(parsed[0].status, GitStatus::Modified);
        assert_eq!(parsed[0].path, Path::new("src/a file"));
        assert_eq!(parsed[1].status, GitStatus::Renamed);
        assert_eq!(parsed[1].path, Path::new("new name"));
        assert_eq!(
            parsed[1].original_path.as_deref(),
            Some(Path::new("old name"))
        );
        assert_eq!(parsed[2].status, GitStatus::Conflict);
        assert_eq!(parsed[3].status, GitStatus::Untracked);
        assert_eq!(parsed[4].status, GitStatus::Deleted);
    }

    #[test]
    fn stale_load_response_is_rejected_after_collapse() {
        let root = temp_dir("stale");
        fs::write(root.join("file"), b"").unwrap();
        let mut sidebar = Sidebar::new(&root).unwrap();
        sidebar.request_expand(&root);
        sidebar.collapse(&root);
        thread::sleep(Duration::from_millis(30));
        sidebar.tick();
        assert_eq!(sidebar.all_visible_rows().len(), 1);
        assert!(!sidebar.all_visible_rows()[0].expanded);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_and_ancestor_cycle_are_not_expandable() {
        use std::os::unix::fs::symlink;
        let root = temp_dir("links");
        let outside = temp_dir("outside");
        symlink(&outside, root.join("escape")).unwrap();
        symlink(&root, root.join("cycle")).unwrap();
        let mut sidebar = Sidebar::new(&root).unwrap();
        sidebar.request_expand(&root);
        wait(&mut sidebar);
        for row in sidebar.all_visible_rows().into_iter().skip(1) {
            assert_eq!(row.kind, EntryKind::Symlink);
            assert!(row.error.is_some());
        }
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn dirty_ancestors_and_deleted_children_are_materialized() {
        let root = temp_dir("git-decoration");
        fs::create_dir(root.join("dir")).unwrap();
        let mut sidebar = Sidebar::new(&root).unwrap();
        sidebar.request_expand(&root);
        wait(&mut sidebar);
        sidebar.request_expand(&root.join("dir"));
        wait(&mut sidebar);
        sidebar.apply_git(vec![GitChange {
            path: PathBuf::from("dir/gone"),
            original_path: None,
            status: GitStatus::Deleted,
        }]);
        let rows = sidebar.all_visible_rows();
        assert!(rows[0].git.dirty_descendant);
        assert!(rows.iter().any(|row| row.kind == EntryKind::Deleted));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn non_git_failure_degrades_to_error() {
        let root = temp_dir("nongit");
        let mut sidebar = Sidebar::new(&root).unwrap();
        assert!(sidebar.request_git_refresh());
        wait(&mut sidebar);
        assert!(sidebar.git_error().is_some());
        fs::remove_dir_all(root).unwrap();
    }
}
