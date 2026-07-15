use std::env;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH, WorkbenchLayoutConfig};

const SCHEMA_VERSION: u32 = 1;
const MAX_SAVED_PANE_SIZE: u16 = 4_096;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The only workbench state that is user intent rather than solved/runtime state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayoutIntent {
    pub config: WorkbenchLayoutConfig,
    pub sidebar_collapsed: bool,
    pub bottom_collapsed: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateFile {
    version: u32,
    workspace_root: String,
    sidebar_width: u16,
    editor_width: Option<u16>,
    bottom_height: u16,
    sidebar_collapsed: bool,
    bottom_collapsed: bool,
}

/// A per-workspace store rooted outside the workspace itself.
#[derive(Debug, Clone)]
pub struct LayoutStore {
    workspace_identity: String,
    path: PathBuf,
}

impl LayoutStore {
    /// Uses `$AMI_CODE_STATE_DIR`, then `$XDG_STATE_HOME/ami-code`. The HOME
    /// fallback is `~/Library/Application Support/ami-code` on macOS and
    /// `~/.local/state/ami-code` elsewhere. Files live below `layouts`.
    pub fn from_environment(workspace_root: &Path) -> Result<Self> {
        let state_root = if let Some(path) = env::var_os("AMI_CODE_STATE_DIR") {
            PathBuf::from(path)
        } else if let Some(path) = env::var_os("XDG_STATE_HOME") {
            PathBuf::from(path).join("ami-code")
        } else if let Some(path) = env::var_os("HOME") {
            home_state_root(PathBuf::from(path))
        } else {
            bail!("no state directory: set AMI_CODE_STATE_DIR, XDG_STATE_HOME, or HOME");
        };
        Ok(Self::new(workspace_root, state_root))
    }

    /// Explicit state roots keep filesystem behavior deterministic in tests.
    pub fn new(workspace_root: &Path, state_root: impl AsRef<Path>) -> Self {
        let workspace_identity = root_identity(workspace_root);
        let key = hex(&Sha256::digest(workspace_identity.as_bytes()));
        Self {
            workspace_identity,
            path: state_root
                .as_ref()
                .join("layouts")
                .join(format!("{key}.json")),
        }
    }

    /// Missing files mean defaults. Every malformed or mismatched file is an
    /// error for the caller to surface nonfatally.
    pub fn load(&self) -> Result<Option<LayoutIntent>> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("failed to read saved layout"),
        };
        let state: StateFile =
            serde_json::from_slice(&bytes).context("invalid saved layout JSON")?;
        if state.version != SCHEMA_VERSION {
            bail!("unsupported saved layout version {}", state.version);
        }
        if state.workspace_root != self.workspace_identity {
            bail!("saved layout belongs to a different workspace");
        }
        let intent = LayoutIntent {
            config: WorkbenchLayoutConfig {
                sidebar_width: state.sidebar_width,
                editor_width: state.editor_width,
                bottom_height: state.bottom_height,
            },
            sidebar_collapsed: state.sidebar_collapsed,
            bottom_collapsed: state.bottom_collapsed,
        };
        validate(intent)?;
        Ok(Some(intent))
    }

    pub fn save(&self, intent: LayoutIntent) -> Result<()> {
        validate(intent)?;
        let parent = self.path.parent().expect("layout path has parent");
        fs::create_dir_all(parent).context("failed to create layout state directory")?;
        let state = StateFile {
            version: SCHEMA_VERSION,
            workspace_root: self.workspace_identity.clone(),
            sidebar_width: intent.config.sidebar_width,
            editor_width: intent.config.editor_width,
            bottom_height: intent.config.bottom_height,
            sidebar_collapsed: intent.sidebar_collapsed,
            bottom_collapsed: intent.bottom_collapsed,
        };
        let bytes = serde_json::to_vec_pretty(&state).context("failed to encode layout state")?;

        let temp = self.unique_temp_path();
        let result = (|| -> Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .context("failed to create temporary layout state")?;
            file.write_all(&bytes)
                .context("failed to write temporary layout state")?;
            file.write_all(b"\n")
                .context("failed to finish temporary layout state")?;
            file.sync_all()
                .context("failed to sync temporary layout state")?;
            drop(file);
            fs::rename(&temp, &self.path).context("failed to atomically replace layout state")?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }

    fn unique_temp_path(&self) -> PathBuf {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = self
            .path
            .file_name()
            .unwrap_or_else(|| OsStr::new("layout"));
        self.path.with_file_name(format!(
            ".{}.{}.{}.tmp",
            name.to_string_lossy(),
            std::process::id(),
            sequence
        ))
    }

    #[cfg(test)]
    fn path(&self) -> &Path {
        &self.path
    }
}

fn validate(intent: LayoutIntent) -> Result<()> {
    validate_dimension(
        "sidebar width",
        intent.config.sidebar_width,
        MIN_TERMINAL_WIDTH,
    )?;
    if let Some(width) = intent.config.editor_width {
        validate_dimension("editor width", width, MIN_TERMINAL_WIDTH)?;
    }
    validate_dimension(
        "bottom height",
        intent.config.bottom_height,
        MIN_TERMINAL_HEIGHT,
    )?;
    Ok(())
}

fn validate_dimension(name: &str, value: u16, minimum: u16) -> Result<()> {
    if value < minimum {
        bail!("saved {name} is below the terminal minimum");
    }
    if value > MAX_SAVED_PANE_SIZE {
        bail!("saved {name} exceeds the supported maximum");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn home_state_root(home: PathBuf) -> PathBuf {
    home.join("Library/Application Support/ami-code")
}

#[cfg(not(target_os = "macos"))]
fn home_state_root(home: PathBuf) -> PathBuf {
    home.join(".local/state/ami-code")
}

#[cfg(unix)]
fn root_identity(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    format!("unix:{}", hex(path.as_os_str().as_bytes()))
}

#[cfg(windows)]
fn root_identity(path: &Path) -> String {
    use std::os::windows::ffi::OsStrExt;
    let bytes: Vec<u8> = path
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect();
    format!("windows:{}", hex(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let path = env::temp_dir().join(format!(
            "ami-code-layout-{label}-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn intent() -> LayoutIntent {
        LayoutIntent {
            config: WorkbenchLayoutConfig {
                sidebar_width: 31,
                editor_width: Some(67),
                bottom_height: 15,
            },
            sidebar_collapsed: true,
            bottom_collapsed: true,
        }
    }

    #[test]
    fn roots_are_isolated_and_all_five_fields_roundtrip() {
        let state = temp_dir("roundtrip");
        let first = LayoutStore::new(Path::new("/canonical/one"), &state);
        let second = LayoutStore::new(Path::new("/canonical/two"), &state);
        assert_ne!(first.path(), second.path());
        first.save(intent()).unwrap();
        assert_eq!(first.load().unwrap(), Some(intent()));
        assert_eq!(second.load().unwrap(), None);
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn corrupt_future_wrong_root_and_invalid_files_are_rejected() {
        let state = temp_dir("invalid");
        let store = LayoutStore::new(Path::new("/canonical/one"), &state);
        fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        for contents in [
            "not json".to_string(),
            serde_json::to_string(&StateFile {
                version: SCHEMA_VERSION + 1,
                workspace_root: store.workspace_identity.clone(),
                sidebar_width: 24,
                editor_width: None,
                bottom_height: 12,
                sidebar_collapsed: false,
                bottom_collapsed: false,
            })
            .unwrap(),
            serde_json::to_string(&StateFile {
                version: SCHEMA_VERSION,
                workspace_root: "another-root".into(),
                sidebar_width: 24,
                editor_width: None,
                bottom_height: 12,
                sidebar_collapsed: false,
                bottom_collapsed: false,
            })
            .unwrap(),
            serde_json::to_string(&StateFile {
                version: SCHEMA_VERSION,
                workspace_root: store.workspace_identity.clone(),
                sidebar_width: 0,
                editor_width: None,
                bottom_height: 12,
                sidebar_collapsed: false,
                bottom_collapsed: false,
            })
            .unwrap(),
            serde_json::to_string(&StateFile {
                version: SCHEMA_VERSION,
                workspace_root: store.workspace_identity.clone(),
                sidebar_width: 24,
                editor_width: Some(MAX_SAVED_PANE_SIZE + 1),
                bottom_height: 12,
                sidebar_collapsed: false,
                bottom_collapsed: false,
            })
            .unwrap(),
        ] {
            fs::write(store.path(), contents).unwrap();
            assert!(store.load().is_err());
        }
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn atomic_save_leaves_no_temporary_file_and_write_failure_is_reported() {
        let state = temp_dir("atomic");
        let store = LayoutStore::new(Path::new("/canonical/one"), &state);
        store.save(intent()).unwrap();
        let entries: Vec<_> = fs::read_dir(store.path().parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![store.path().file_name().unwrap()]);

        let blocked = state.join("blocked");
        fs::write(&blocked, b"not a directory").unwrap();
        let failing = LayoutStore::new(Path::new("/canonical/two"), &blocked);
        assert!(failing.save(intent()).is_err());
        fs::remove_dir_all(state).unwrap();
    }
}
