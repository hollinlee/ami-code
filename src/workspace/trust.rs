use std::ffi::OsString;
use std::fs;
#[cfg(unix)]
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const STORE_VERSION: u32 = 1;

/// The trust decision that applies to the workspace's current filesystem object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceTrustState {
    /// There is no decision, or the current identity has explicitly been revoked.
    Untrusted,
    /// The current filesystem object has explicitly been trusted.
    Trusted,
    /// A decision exists for this path, but the path now names a different object.
    Stale,
}

/// A cloneable handle to the application-owned trust record for one workspace.
///
/// The handle retains the canonical root path, but resolves its filesystem identity
/// afresh on every operation so a runtime can safely use it across Pi generations.
#[derive(Debug, Clone)]
pub struct WorkspaceTrustStore {
    root: PathBuf,
    root_identity: RootIdentity,
    directory: PathBuf,
    record_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "encoding", content = "components")]
enum RootIdentity {
    #[cfg(unix)]
    UnixBytes(Vec<u8>),
    #[cfg(windows)]
    WindowsWide(Vec<u16>),
    #[cfg(not(any(unix, windows)))]
    PlatformBytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum FilesystemIdentity {
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
    #[cfg(windows)]
    Windows { volume: u32, file_index: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum StoredDecision {
    Trust,
    Revoke,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustRecord {
    version: u32,
    root: RootIdentity,
    filesystem: FilesystemIdentity,
    decision: StoredDecision,
}

impl WorkspaceTrustStore {
    /// Locate the app state directory from `AMI_CODE_STATE_DIR`,
    /// `XDG_STATE_HOME`, or the platform home default.
    pub fn from_environment(root: impl AsRef<Path>) -> Result<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let state_root = application_state_root(home.as_deref())?;
        Self::new(root, state_root)
    }

    /// Open the trust store for `root` below `<application_state_root>/trust`.
    pub fn new(root: impl AsRef<Path>, application_state_root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize().with_context(|| {
            format!(
                "failed to resolve workspace root for trust: {}",
                root.as_ref().display()
            )
        })?;
        let metadata = fs::metadata(&root)
            .with_context(|| format!("failed to inspect workspace root: {}", root.display()))?;
        if !metadata.is_dir() {
            bail!(
                "workspace trust root is not a directory: {}",
                root.display()
            );
        }

        let state_root = resolved_destination(application_state_root.as_ref())?;
        let directory = state_root.join("trust");
        if directory.starts_with(&root) {
            bail!(
                "workspace trust store must not be inside the workspace: {}",
                directory.display()
            );
        }
        ensure_private_directory(&directory)?;

        let root_identity = root_identity(&root);
        let key = root_key(&root_identity)?;
        let record_path = directory.join(format!("{key}.json"));
        Ok(Self {
            root,
            root_identity,
            directory,
            record_path,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve the saved decision against the root's identity at this instant.
    pub fn resolve(&self) -> Result<WorkspaceTrustState> {
        let bytes = match read_record_file(&self.record_path)? {
            Some(bytes) => bytes,
            None => return Ok(WorkspaceTrustState::Untrusted),
        };
        let record: TrustRecord = serde_json::from_slice(&bytes).with_context(|| {
            format!(
                "workspace trust record is corrupt: {}",
                self.record_path.display()
            )
        })?;
        if record.version != STORE_VERSION {
            bail!(
                "unsupported workspace trust record version {} in {}",
                record.version,
                self.record_path.display()
            );
        }
        if record.root != self.root_identity {
            bail!(
                "workspace trust record belongs to a different root: {}",
                self.record_path.display()
            );
        }
        if record.filesystem != filesystem_identity(&self.root)? {
            return Ok(WorkspaceTrustState::Stale);
        }
        Ok(match record.decision {
            StoredDecision::Trust => WorkspaceTrustState::Trusted,
            StoredDecision::Revoke => WorkspaceTrustState::Untrusted,
        })
    }

    /// Persist an explicit trust decision for the root's current identity.
    pub fn trust(&self) -> Result<()> {
        self.write_decision(StoredDecision::Trust)
    }

    /// Persist an explicit revoke decision for the root's current identity.
    pub fn revoke(&self) -> Result<()> {
        self.write_decision(StoredDecision::Revoke)
    }

    fn write_decision(&self, decision: StoredDecision) -> Result<()> {
        ensure_private_directory(&self.directory)?;
        validate_replace_target(&self.record_path)?;
        let record = TrustRecord {
            version: STORE_VERSION,
            root: self.root_identity.clone(),
            filesystem: filesystem_identity(&self.root)?,
            decision,
        };
        let mut contents =
            serde_json::to_vec_pretty(&record).context("failed to encode trust record")?;
        contents.push(b'\n');

        let mut temporary = tempfile::Builder::new()
            .prefix(".ami-trust-")
            .tempfile_in(&self.directory)
            .with_context(|| {
                format!(
                    "failed to create trust record in {}",
                    self.directory.display()
                )
            })?;
        set_private_mode(temporary.path(), 0o600)?;
        temporary
            .write_all(&contents)
            .context("failed to write workspace trust record")?;
        temporary
            .as_file()
            .sync_all()
            .context("failed to sync workspace trust record")?;
        temporary
            .persist(&self.record_path)
            .map_err(|error| error.error)
            .with_context(|| {
                format!(
                    "failed to atomically replace {}",
                    self.record_path.display()
                )
            })?;
        set_private_mode(&self.record_path, 0o600)?;
        sync_directory(&self.directory)?;
        Ok(())
    }
}

fn application_state_root(home: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = nonempty_env("AMI_CODE_STATE_DIR") {
        return absolute_path(PathBuf::from(path));
    }
    if let Some(path) = nonempty_env("XDG_STATE_HOME") {
        return absolute_path(PathBuf::from(path).join("ami-code"));
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = home {
        return Ok(home.join("Library/Application Support/ami-code"));
    }
    home.map(|path| path.join(".local/state/ami-code"))
        .context("no trust state location: set AMI_CODE_STATE_DIR, XDG_STATE_HOME, or HOME")
}

fn nonempty_env(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

fn absolute_path(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()
            .context("failed to resolve relative trust state path")?
            .join(path))
    }
}

/// Canonicalize the existing portion without creating anything. This catches a
/// state path whose existing ancestor is a symlink into the workspace.
fn resolved_destination(path: &Path) -> Result<PathBuf> {
    let absolute = absolute_path(path.to_path_buf())?;
    let mut existing = absolute.as_path();
    while !existing.exists() {
        existing = existing
            .parent()
            .context("trust state path has no existing ancestor")?;
    }
    let canonical = existing
        .canonicalize()
        .with_context(|| format!("failed to resolve trust state path: {}", existing.display()))?;
    let suffix = absolute
        .strip_prefix(existing)
        .context("failed to resolve trust state suffix")?;
    Ok(canonical.join(suffix))
}

fn root_identity(path: &Path) -> RootIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        RootIdentity::UnixBytes(path.as_os_str().as_bytes().to_vec())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        RootIdentity::WindowsWide(path.as_os_str().encode_wide().collect())
    }
    #[cfg(not(any(unix, windows)))]
    {
        RootIdentity::PlatformBytes(path.as_os_str().as_encoded_bytes().to_vec())
    }
}

fn root_key(identity: &RootIdentity) -> Result<String> {
    let encoded =
        serde_json::to_vec(identity).context("failed to encode workspace root identity")?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn filesystem_identity(path: &Path) -> Result<FilesystemIdentity> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to inspect workspace identity: {}", path.display()))?;
    if !metadata.is_dir() {
        bail!(
            "workspace root is no longer a directory: {}",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(FilesystemIdentity::Unix {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(FilesystemIdentity::Windows {
            volume: metadata
                .volume_serial_number()
                .context("workspace volume identity is unavailable")?,
            file_index: metadata
                .file_index()
                .context("workspace file identity is unavailable")?,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = metadata;
        bail!("stable workspace filesystem identity is unsupported on this platform")
    }
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                bail!("trust store has an unsafe file type: {}", path.display());
            }
            validate_private_mode(path, &metadata, 0o077)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)
                .with_context(|| format!("failed to create trust store: {}", path.display()))?;
            set_private_mode(path, 0o700)?;
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect trust store: {}", path.display()));
        }
    }
    Ok(())
}

fn read_record_file(path: &Path) -> Result<Option<Vec<u8>>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect trust record: {}", path.display()));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        bail!("trust record has an unsafe file type: {}", path.display());
    }
    validate_private_mode(path, &metadata, 0o077)?;
    fs::read(path)
        .map(Some)
        .with_context(|| format!("failed to read trust record: {}", path.display()))
}

fn validate_replace_target(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                bail!("trust record has an unsafe file type: {}", path.display());
            }
            validate_private_mode(path, &metadata, 0o077)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("failed to inspect trust record: {}", path.display())),
    }
}

#[cfg(unix)]
fn validate_private_mode(path: &Path, metadata: &fs::Metadata, forbidden: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & forbidden != 0 {
        bail!("trust path has insecure permissions: {}", path.display());
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_private_mode(_path: &Path, _metadata: &fs::Metadata, _forbidden: u32) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("failed to secure trust path: {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("failed to sync trust store: {}", path.display()))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, PathBuf, PathBuf) {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path().join("workspace");
        let state = temp.path().join("state");
        fs::create_dir(&workspace).unwrap();
        (temp, workspace, state)
    }

    #[test]
    fn defaults_untrusted_and_trust_revoke_roundtrip() {
        let (_temp, workspace, state) = fixture();
        let store = WorkspaceTrustStore::new(&workspace, &state).unwrap();
        assert_eq!(store.resolve().unwrap(), WorkspaceTrustState::Untrusted);
        store.trust().unwrap();
        assert_eq!(store.resolve().unwrap(), WorkspaceTrustState::Trusted);
        store.revoke().unwrap();
        assert_eq!(store.resolve().unwrap(), WorkspaceTrustState::Untrusted);
    }

    #[test]
    fn roots_are_path_isolated_and_store_is_outside_workspace() {
        let (temp, one, state) = fixture();
        let two = temp.path().join("workspace-two");
        fs::create_dir(&two).unwrap();
        let first = WorkspaceTrustStore::new(&one, &state).unwrap();
        let second = WorkspaceTrustStore::new(&two, &state).unwrap();
        first.trust().unwrap();
        assert_eq!(second.resolve().unwrap(), WorkspaceTrustState::Untrusted);
        assert_ne!(first.record_path, second.record_path);
        assert!(!first.record_path.starts_with(&one));
        assert!(WorkspaceTrustStore::new(&one, one.join(".state")).is_err());
    }

    #[test]
    fn recreating_the_same_path_makes_saved_decision_stale() {
        let (temp, workspace, state) = fixture();
        let store = WorkspaceTrustStore::new(&workspace, &state).unwrap();
        store.trust().unwrap();
        let old = temp.path().join("old-workspace");
        fs::rename(&workspace, &old).unwrap();
        fs::create_dir(&workspace).unwrap();
        assert_eq!(store.resolve().unwrap(), WorkspaceTrustState::Stale);
        store.revoke().unwrap();
        assert_eq!(store.resolve().unwrap(), WorkspaceTrustState::Untrusted);
    }

    #[test]
    fn corrupt_future_and_wrong_root_records_fail_closed() {
        let (temp, workspace, state) = fixture();
        let store = WorkspaceTrustStore::new(&workspace, &state).unwrap();
        store.trust().unwrap();

        fs::write(&store.record_path, b"not json").unwrap();
        assert!(store.resolve().unwrap_err().to_string().contains("corrupt"));

        let mut record = TrustRecord {
            version: STORE_VERSION + 1,
            root: store.root_identity.clone(),
            filesystem: filesystem_identity(&workspace).unwrap(),
            decision: StoredDecision::Trust,
        };
        fs::write(&store.record_path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(store.resolve().unwrap_err().to_string().contains("version"));

        let other = temp.path().join("other");
        fs::create_dir(&other).unwrap();
        record.version = STORE_VERSION;
        record.root = root_identity(&other.canonicalize().unwrap());
        fs::write(&store.record_path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(
            store
                .resolve()
                .unwrap_err()
                .to_string()
                .contains("different root")
        );
    }

    #[cfg(unix)]
    #[test]
    fn records_are_atomic_private_regular_files_and_unsafe_types_fail() {
        use std::os::unix::fs::PermissionsExt;

        let (temp, workspace, state) = fixture();
        let store = WorkspaceTrustStore::new(&workspace, &state).unwrap();
        store.trust().unwrap();
        assert_eq!(
            fs::metadata(&store.directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&store.record_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(
            fs::symlink_metadata(&store.record_path)
                .unwrap()
                .file_type()
                .is_file()
        );
        assert_eq!(fs::read_dir(&store.directory).unwrap().count(), 1);

        fs::remove_file(&store.record_path).unwrap();
        let outside = temp.path().join("outside");
        fs::write(&outside, b"untouched").unwrap();
        std::os::unix::fs::symlink(&outside, &store.record_path).unwrap();
        assert!(store.resolve().is_err());
        assert!(store.trust().is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"untouched");
    }
}
