use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use super::{BackendKind, BackendSpec, build_backend_process_spec};
use crate::terminal::ProcessSpec;
use crate::workspace::{Workspace, WorkspaceTrustState};

const PROFILE_VERSION: &str = "managed-v1";
const SETTINGS: &[u8] = b"{}\n";
const KEYBINDINGS: &[u8] = b"{}\n";

#[derive(Debug, Clone, Copy, Default)]
struct PiBackend;

impl BackendSpec for PiBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Agent
    }

    fn display_name(&self) -> &str {
        "pi"
    }

    fn process_spec(&self, workspace: &Workspace) -> ProcessSpec {
        build_backend_process_spec(self.display_name(), self.display_name(), workspace)
    }
}

/// A materialized, versioned Pi home. This value contains paths only; auth data is
/// deliberately never opened, copied, or represented in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPiProfile {
    root: PathBuf,
}

impl ManagedPiProfile {
    /// Resolve the user's original Pi home before replacing `PI_CODING_AGENT_DIR`
    /// in the child process and materialize the managed profile.
    pub fn from_environment() -> Result<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let state = managed_state_root(home.as_deref())?;
        let source_dir = match std::env::var_os("PI_CODING_AGENT_DIR") {
            Some(value) if !value.is_empty() => absolute_path(PathBuf::from(value))?,
            Some(_) => bail!("PI_CODING_AGENT_DIR is empty"),
            None => home
                .map(|path| path.join(".pi/agent"))
                .context("HOME is not set; cannot locate the shared Pi auth file")?,
        };
        Self::materialize(state, source_dir.join("auth.json"))
    }

    /// Materialize a profile under an explicit application state root. This is
    /// also the non-environment entry point used by tests and embedders.
    pub fn materialize(
        application_state_root: impl AsRef<Path>,
        auth_source: impl AsRef<Path>,
    ) -> Result<Self> {
        let application_state_root = application_state_root.as_ref();
        application_state_root
            .to_str()
            .context("managed Pi state path is not valid UTF-8")?;
        let root = application_state_root.join("pi").join(PROFILE_VERSION);
        let auth_source = absolute_path(auth_source.as_ref().to_path_buf())?;

        ensure_private_dir(&application_state_root.join("pi"))?;
        ensure_private_dir(&root)?;
        ensure_private_dir(&root.join("sessions"))?;
        for resource in ["extensions", "skills", "prompts", "themes"] {
            ensure_private_dir(&root.join(resource))?;
        }
        write_managed_file(&root.join("settings.json"), SETTINGS)?;
        write_managed_file(&root.join("keybindings.json"), KEYBINDINGS)?;
        ensure_auth_source(&auth_source, &root)?;
        ensure_auth_link(&root.join("auth.json"), &auth_source)?;

        Ok(Self { root })
    }

    #[cfg(test)]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn session_dir(&self, workspace: &Workspace) -> PathBuf {
        let digest = Sha256::digest(workspace.root().as_os_str().as_encoded_bytes());
        self.root.join("sessions").join(format!("{digest:x}"))
    }

    /// Build a locked-down spec for compatibility with callers that have not yet
    /// integrated trust resolution. This method never implicitly trusts a project.
    pub fn process_spec(&self, workspace: &Workspace) -> Result<ProcessSpec> {
        self.process_spec_with_trust(workspace, WorkspaceTrustState::Untrusted)
    }

    /// Build a one-run Pi spec using the trust decision resolved for this exact
    /// workspace generation.
    pub fn process_spec_with_trust(
        &self,
        workspace: &Workspace,
        trust: WorkspaceTrustState,
    ) -> Result<ProcessSpec> {
        // Workspace roots are canonicalized by Workspace, making this hash stable
        // for aliases while keeping different roots isolated.
        let session_dir = self.session_dir(workspace);
        ensure_private_dir(&session_dir)?;
        debug_assert_eq!(PiBackend.kind(), BackendKind::Agent);
        let profile_dir = self
            .root
            .to_str()
            .context("managed Pi profile path is not valid UTF-8")?;
        let session_dir_arg = session_dir
            .to_str()
            .context("managed Pi session path is not valid UTF-8")?;
        let mut spec = PiBackend
            .process_spec(workspace)
            .env("PI_CODING_AGENT_DIR", profile_dir);
        spec.args = vec![
            "--session-dir".to_string(),
            session_dir_arg.to_owned(),
            match trust {
                WorkspaceTrustState::Trusted => "--approve".to_string(),
                WorkspaceTrustState::Untrusted | WorkspaceTrustState::Stale => {
                    "--no-approve".to_string()
                }
            },
        ];
        if trust != WorkspaceTrustState::Trusted {
            spec.args.extend([
                "--no-extensions".to_string(),
                "--no-skills".to_string(),
                "--no-prompt-templates".to_string(),
                "--no-themes".to_string(),
            ]);
        }
        Ok(spec)
    }
}

fn managed_state_root(home: Option<&Path>) -> Result<PathBuf> {
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
        .context("no managed state location: set AMI_CODE_STATE_DIR, XDG_STATE_HOME, or HOME")
}

fn nonempty_env(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

fn absolute_path(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()
            .context("failed to resolve relative managed Pi path")?
            .join(path))
    }
}

fn ensure_private_dir(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() => {
            bail!(
                "managed Pi directory has an unsafe file type: {}",
                path.display()
            )
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir_all(path)
            .with_context(|| {
                format!("failed to create managed Pi directory: {}", path.display())
            })?,
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to inspect managed Pi directory: {}", path.display())
            });
        }
    }
    set_private_mode(path, 0o700)?;
    Ok(())
}

fn write_managed_file(path: &Path, contents: &[u8]) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            bail!(
                "managed Pi asset has an unsafe file type: {}",
                path.display()
            )
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to inspect managed Pi asset: {}", path.display())
            });
        }
    }
    let parent = path.parent().context("managed Pi asset has no parent")?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".ami-pi-asset-")
        .tempfile_in(parent)
        .with_context(|| format!("failed to create managed Pi asset: {}", path.display()))?;
    temporary
        .write_all(contents)
        .with_context(|| format!("failed to write managed Pi asset: {}", path.display()))?;
    temporary
        .as_file()
        .sync_all()
        .with_context(|| format!("failed to sync managed Pi asset: {}", path.display()))?;
    set_private_mode(temporary.path(), 0o600)?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace managed Pi asset: {}", path.display()))?;
    set_private_mode(path, 0o600)
}

#[cfg(unix)]
fn set_private_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("failed to secure managed Pi path: {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

fn ensure_auth_source(source: &Path, managed_root: &Path) -> Result<()> {
    if let Some(parent) = source.parent() {
        match fs::symlink_metadata(parent) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                bail!(
                    "shared Pi auth parent must be a non-symlink directory: {}",
                    parent.display()
                )
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "failed to create shared Pi auth directory: {}",
                        parent.display()
                    )
                })?;
                set_private_mode(parent, 0o700)?;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to inspect shared Pi auth directory: {}",
                        parent.display()
                    )
                });
            }
        }
    }

    if !source.exists() {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(source) {
            Ok(mut file) => {
                file.write_all(b"{}\n").with_context(|| {
                    format!(
                        "failed to initialize shared Pi auth file: {}",
                        source.display()
                    )
                })?;
                file.sync_all().with_context(|| {
                    format!("failed to sync shared Pi auth file: {}", source.display())
                })?;
                set_private_mode(source, 0o600)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to create shared Pi auth file: {}", source.display())
                });
            }
        }
    }
    validate_auth_source(source, managed_root)
}

fn validate_auth_source(source: &Path, managed_root: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("shared Pi auth file is unavailable: {}", source.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        bail!(
            "shared Pi auth path must be a regular, non-symlink file: {}",
            source.display()
        );
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!(
                "shared Pi auth file has insecure group/other permissions: {}",
                source.display()
            );
        }
        let parent = source
            .parent()
            .context("shared Pi auth path has no parent")?;
        let parent_metadata = fs::symlink_metadata(parent).with_context(|| {
            format!(
                "failed to inspect shared Pi auth directory: {}",
                parent.display()
            )
        })?;
        let managed_metadata = fs::symlink_metadata(managed_root)
            .context("failed to inspect managed Pi profile owner")?;
        if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
            bail!(
                "shared Pi auth parent must be a non-symlink directory: {}",
                parent.display()
            );
        }
        if parent_metadata.permissions().mode() & 0o022 != 0 {
            bail!(
                "shared Pi auth directory has insecure group/other write permissions: {}",
                parent.display()
            );
        }
        if !owners_match(
            metadata.uid(),
            parent_metadata.uid(),
            managed_metadata.uid(),
        ) {
            bail!(
                "shared Pi auth owner does not match the current managed profile owner: {}",
                source.display()
            );
        }
    }
    Ok(())
}

#[cfg(unix)]
fn owners_match(file: u32, source_directory: u32, managed_profile: u32) -> bool {
    file == source_directory && file == managed_profile
}

fn ensure_auth_link(link: &Path, expected: &Path) -> Result<()> {
    match fs::symlink_metadata(link) {
        Ok(metadata) => {
            if !metadata.file_type().is_symlink() {
                bail!(
                    "managed Pi auth entry must be a symlink (credential copies are refused): {}",
                    link.display()
                );
            }
            let target = fs::read_link(link).with_context(|| {
                format!(
                    "failed to validate managed Pi auth link: {}",
                    link.display()
                )
            })?;
            if target != expected {
                bail!(
                    "managed Pi auth link does not target the configured shared auth file: {}",
                    link.display()
                );
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_auth_link(expected, link)
        }
        Err(error) => Err(error).with_context(|| {
            format!(
                "failed to inspect managed Pi auth entry: {}",
                link.display()
            )
        }),
    }
}

#[cfg(unix)]
fn create_auth_link(source: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(source, link)
        .with_context(|| format!("failed to create managed Pi auth link: {}", link.display()))
}

#[cfg(windows)]
fn create_auth_link(source: &Path, link: &Path) -> Result<()> {
    std::os::windows::fs::symlink_file(source, link)
        .with_context(|| format!("failed to create managed Pi auth link: {}", link.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn source(temp: &TempDir) -> PathBuf {
        let dir = temp.path().join("user-agent");
        fs::create_dir(&dir).unwrap();
        let auth = dir.join("auth.json");
        fs::write(&auth, b"TOP-SECRET-MARKER").unwrap();
        set_private_mode(&auth, 0o600).unwrap();
        auth
    }

    #[test]
    fn creates_exact_auth_link_and_private_versioned_assets() {
        let temp = TempDir::new().unwrap();
        let auth = source(&temp);
        let profile = ManagedPiProfile::materialize(temp.path().join("state"), &auth).unwrap();
        assert!(profile.root().ends_with("pi/managed-v1"));
        assert_eq!(
            fs::read_link(profile.root().join("auth.json")).unwrap(),
            auth
        );
        assert_eq!(
            fs::read(profile.root().join("settings.json")).unwrap(),
            SETTINGS
        );
        assert_eq!(
            fs::read(profile.root().join("keybindings.json")).unwrap(),
            KEYBINDINGS
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(profile.root()).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(profile.root().join("settings.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let debug = format!("{profile:?}");
        assert!(!debug.contains("TOP-SECRET-MARKER"));
    }

    #[test]
    fn first_run_creates_empty_shared_auth_and_writes_through_symlink() {
        let temp = TempDir::new().unwrap();
        let auth = temp.path().join("new-agent/auth.json");
        let profile = ManagedPiProfile::materialize(temp.path().join("state"), &auth).unwrap();
        let link = profile.root().join("auth.json");
        assert_eq!(fs::read(&auth).unwrap(), b"{}\n");
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );

        // Pi 0.80.x FileAuthStorageBackend uses in-place writeFileSync, which
        // follows this link instead of replacing it.
        fs::write(&link, b"UPDATED-AUTH-MARKER").unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&auth).unwrap(), b"UPDATED-AUTH-MARKER");
    }

    #[test]
    fn atomic_assets_replace_symlinks_without_touching_their_targets() {
        let temp = TempDir::new().unwrap();
        let auth = source(&temp);
        let state = temp.path().join("state");
        let root = state.join("pi/managed-v1");
        fs::create_dir_all(&root).unwrap();
        let outside = temp.path().join("outside-settings");
        fs::write(&outside, b"DO-NOT-TOUCH").unwrap();
        create_auth_link(&outside, &root.join("settings.json")).unwrap();

        let profile = ManagedPiProfile::materialize(&state, auth).unwrap();
        assert_eq!(fs::read(&outside).unwrap(), b"DO-NOT-TOUCH");
        assert!(
            fs::symlink_metadata(profile.root().join("settings.json"))
                .unwrap()
                .file_type()
                .is_file()
        );
        assert_eq!(
            fs::read(profile.root().join("settings.json")).unwrap(),
            SETTINGS
        );
    }

    #[test]
    fn rejects_auth_copy_wrong_link_and_wrong_source_type_without_reading_data() {
        for case in ["copy", "wrong-link", "source-link"] {
            let temp = TempDir::new().unwrap();
            let auth = source(&temp);
            let state = temp.path().join("state");
            let root = state.join("pi/managed-v1");
            fs::create_dir_all(&root).unwrap();
            match case {
                "copy" => fs::write(root.join("auth.json"), b"TOP-SECRET-MARKER").unwrap(),
                "wrong-link" => create_auth_link(temp.path(), &root.join("auth.json")).unwrap(),
                "source-link" => {
                    let real = auth;
                    let linked = temp.path().join("linked-auth");
                    create_auth_link(&real, &linked).unwrap();
                    let error = ManagedPiProfile::materialize(&state, linked)
                        .unwrap_err()
                        .to_string();
                    assert!(!error.contains("TOP-SECRET-MARKER"));
                    continue;
                }
                _ => unreachable!(),
            }
            let error = ManagedPiProfile::materialize(&state, auth)
                .unwrap_err()
                .to_string();
            assert!(!error.contains("TOP-SECRET-MARKER"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn owner_validation_rejects_either_mismatch() {
        assert!(owners_match(501, 501, 501));
        assert!(!owners_match(501, 502, 501));
        assert!(!owners_match(501, 501, 502));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_insecure_auth_mode() {
        let temp = TempDir::new().unwrap();
        let auth = source(&temp);
        set_private_mode(&auth, 0o640).unwrap();
        let error = ManagedPiProfile::materialize(temp.path().join("state"), auth).unwrap_err();
        assert!(error.to_string().contains("insecure"));
        assert!(!format!("{error:?}").contains("TOP-SECRET-MARKER"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_group_or_world_writable_auth_parent() {
        let temp = TempDir::new().unwrap();
        let auth = source(&temp);
        set_private_mode(auth.parent().unwrap(), 0o775).unwrap();
        let error = ManagedPiProfile::materialize(temp.path().join("state"), auth).unwrap_err();
        assert!(error.to_string().contains("directory has insecure"));
        assert!(!format!("{error:?}").contains("TOP-SECRET-MARKER"));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_profile_path_is_rejected_without_lossy_rewrite() {
        use std::os::unix::ffi::OsStringExt;

        let temp = TempDir::new().unwrap();
        let auth = source(&temp);
        let mut bytes = temp.path().as_os_str().as_encoded_bytes().to_vec();
        bytes.extend_from_slice(b"/state-");
        bytes.push(0xff);
        let state = PathBuf::from(std::ffi::OsString::from_vec(bytes));
        assert!(
            ManagedPiProfile::materialize(state, auth)
                .unwrap_err()
                .to_string()
                .contains("not valid UTF-8")
        );
    }

    #[test]
    fn workspace_sessions_are_stable_and_isolated_and_untrusted_spec_is_locked_down() {
        let temp = TempDir::new().unwrap();
        let auth = source(&temp);
        let profile = ManagedPiProfile::materialize(temp.path().join("state"), auth).unwrap();
        let one_path = temp.path().join("one");
        let two_path = temp.path().join("two");
        fs::create_dir(&one_path).unwrap();
        fs::create_dir(&two_path).unwrap();
        let one = Workspace::discover(one_path).unwrap();
        let one_again = Workspace::discover(one.root()).unwrap();
        let two = Workspace::discover(two_path).unwrap();
        assert_eq!(profile.session_dir(&one), profile.session_dir(&one_again));
        assert_ne!(profile.session_dir(&one), profile.session_dir(&two));

        for trust in [WorkspaceTrustState::Untrusted, WorkspaceTrustState::Stale] {
            let spec = profile.process_spec_with_trust(&one, trust).unwrap();
            assert_eq!(spec.cwd.as_deref(), Some(one.root()));
            assert_eq!(
                spec.env.get("PI_CODING_AGENT_DIR").map(PathBuf::from),
                Some(profile.root.clone())
            );
            for flag in [
                "--session-dir",
                "--no-approve",
                "--no-extensions",
                "--no-skills",
                "--no-prompt-templates",
                "--no-themes",
            ] {
                assert!(spec.args.iter().any(|arg| arg == flag));
            }
            assert!(!spec.args.iter().any(|arg| arg == "--approve"));
            assert!(!spec.args.iter().any(|arg| arg == "--no-context-files"));
            assert!(!format!("{spec:?}").contains("TOP-SECRET-MARKER"));
        }

        // The compatibility entry point remains fail-closed.
        assert!(
            profile
                .process_spec(&one)
                .unwrap()
                .args
                .iter()
                .any(|arg| arg == "--no-approve")
        );
    }

    #[test]
    fn trusted_spec_approves_and_allows_project_resource_discovery() {
        let temp = TempDir::new().unwrap();
        let auth = source(&temp);
        let profile = ManagedPiProfile::materialize(temp.path().join("state"), auth).unwrap();
        let workspace_path = temp.path().join("workspace");
        fs::create_dir(&workspace_path).unwrap();
        let workspace = Workspace::discover(workspace_path).unwrap();

        let spec = profile
            .process_spec_with_trust(&workspace, WorkspaceTrustState::Trusted)
            .unwrap();
        assert!(spec.args.iter().any(|arg| arg == "--approve"));
        for blocked in [
            "--no-approve",
            "--no-extensions",
            "--no-skills",
            "--no-prompt-templates",
            "--no-themes",
            "--no-context-files",
        ] {
            assert!(!spec.args.iter().any(|arg| arg == blocked));
        }
        assert!(spec.args.iter().any(|arg| arg == "--session-dir"));
        assert_eq!(
            spec.env.get("PI_CODING_AGENT_DIR").map(PathBuf::from),
            Some(profile.root.clone())
        );
    }
}
