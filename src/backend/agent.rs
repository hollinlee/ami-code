use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::{BackendKind, BackendSpec, build_backend_process_spec};
use crate::terminal::ProcessSpec;
use crate::workspace::{Workspace, WorkspaceTrustState, workspace_generation_key};

// Keep the original storage path so existing workspace sessions remain available.
const LEGACY_PROFILE_VERSION: &str = "managed-v1";

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

/// Native Pi launch policy plus ami-code-owned workspace session storage.
///
/// Pi configuration and credentials remain owned by the user's environment. This
/// type only creates private session directories and builds trust-scoped argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativePiProfile {
    application_state_root: PathBuf,
}

impl NativePiProfile {
    pub fn from_environment() -> Result<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        Self::new(application_state_root(home.as_deref())?)
    }

    pub fn new(application_state_root: impl AsRef<Path>) -> Result<Self> {
        let application_state_root = absolute_path(application_state_root.as_ref().to_path_buf())?;
        application_state_root
            .to_str()
            .context("native Pi state path is not valid UTF-8")?;
        Ok(Self {
            application_state_root,
        })
    }

    #[cfg(test)]
    pub fn session_root(&self) -> PathBuf {
        self.application_state_root
            .join("pi")
            .join(LEGACY_PROFILE_VERSION)
            .join("sessions")
    }

    pub fn session_dir(&self, workspace: &Workspace) -> Result<PathBuf> {
        Ok(self
            .application_state_root
            .join("pi")
            .join(LEGACY_PROFILE_VERSION)
            .join("sessions")
            .join(workspace_generation_key(workspace.root())?))
    }

    pub fn process_spec_with_trust(
        &self,
        workspace: &Workspace,
        trust: WorkspaceTrustState,
    ) -> Result<ProcessSpec> {
        let pi_root = self.application_state_root.join("pi");
        let legacy_root = pi_root.join(LEGACY_PROFILE_VERSION);
        let session_root = legacy_root.join("sessions");
        for directory in [&pi_root, &legacy_root, &session_root] {
            ensure_private_dir(directory)?;
        }
        let session_dir = self.session_dir(workspace)?;
        ensure_private_dir(&session_dir)?;
        debug_assert_eq!(PiBackend.kind(), BackendKind::Agent);
        let session_dir_arg = session_dir
            .to_str()
            .context("native Pi session path is not valid UTF-8")?;
        let mut spec = PiBackend.process_spec(workspace);
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
        Ok(spec)
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
    home.map(|path| path.join(".local/state/ami-code")).context(
        "no native Pi session state location: set AMI_CODE_STATE_DIR, XDG_STATE_HOME, or HOME",
    )
}

fn nonempty_env(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

fn absolute_path(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()
            .context("failed to resolve relative native Pi state path")?
            .join(path))
    }
}

fn ensure_private_dir(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() => {
            bail!(
                "native Pi session directory has an unsafe file type: {}",
                path.display()
            )
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir_all(path)
            .with_context(|| {
                format!(
                    "failed to create native Pi session directory: {}",
                    path.display()
                )
            })?,
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to inspect native Pi session directory: {}",
                    path.display()
                )
            });
        }
    }
    set_private_mode(path, 0o700)?;
    Ok(())
}

#[cfg(unix)]
fn set_private_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).with_context(|| {
        format!(
            "failed to secure native Pi session path: {}",
            path.display()
        )
    })
}

#[cfg(not(unix))]
fn set_private_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn keeps_legacy_session_root_without_materializing_profile_assets() {
        let temp = TempDir::new().unwrap();
        let state = temp.path().join("state");
        let profile = NativePiProfile::new(&state).unwrap();
        let workspace_path = temp.path().join("workspace");
        fs::create_dir(&workspace_path).unwrap();
        let workspace = Workspace::discover(workspace_path).unwrap();

        let spec = profile
            .process_spec_with_trust(&workspace, WorkspaceTrustState::Untrusted)
            .unwrap();

        assert!(profile.session_root().ends_with("pi/managed-v1/sessions"));
        assert!(profile.session_dir(&workspace).unwrap().is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for directory in [
                state.join("pi"),
                state.join("pi/managed-v1"),
                profile.session_root(),
                profile.session_dir(&workspace).unwrap(),
            ] {
                assert_eq!(
                    fs::metadata(directory).unwrap().permissions().mode() & 0o777,
                    0o700
                );
            }
        }
        for asset in [
            "settings.json",
            "keybindings.json",
            "auth.json",
            "extensions",
            "skills",
            "prompts",
            "themes",
        ] {
            assert!(!state.join("pi/managed-v1").join(asset).exists());
        }
        assert!(!spec.env.contains_key("PI_CODING_AGENT_DIR"));
    }

    #[test]
    fn workspace_sessions_are_stable_and_generation_isolated() {
        let temp = TempDir::new().unwrap();
        let profile = NativePiProfile::new(temp.path().join("state")).unwrap();
        let one_path = temp.path().join("one");
        let two_path = temp.path().join("two");
        fs::create_dir(&one_path).unwrap();
        fs::create_dir(&two_path).unwrap();
        let one = Workspace::discover(&one_path).unwrap();
        let one_again = Workspace::discover(one.root()).unwrap();
        let two = Workspace::discover(&two_path).unwrap();

        assert_eq!(
            profile.session_dir(&one).unwrap(),
            profile.session_dir(&one_again).unwrap()
        );
        assert_ne!(
            profile.session_dir(&one).unwrap(),
            profile.session_dir(&two).unwrap()
        );

        let original = profile.session_dir(&one).unwrap();
        fs::rename(&one_path, temp.path().join("old-one")).unwrap();
        fs::create_dir(&one_path).unwrap();
        let replacement = Workspace::discover(&one_path).unwrap();
        assert_ne!(original, profile.session_dir(&replacement).unwrap());
    }

    #[test]
    fn trust_only_controls_project_approval() {
        let temp = TempDir::new().unwrap();
        let profile = NativePiProfile::new(temp.path().join("state")).unwrap();
        let workspace_path = temp.path().join("workspace");
        fs::create_dir(&workspace_path).unwrap();
        let workspace = Workspace::discover(workspace_path).unwrap();

        let session_dir = profile.session_dir(&workspace).unwrap();
        let session_dir = session_dir.to_str().unwrap().to_string();
        for trust in [WorkspaceTrustState::Untrusted, WorkspaceTrustState::Stale] {
            let spec = profile.process_spec_with_trust(&workspace, trust).unwrap();
            assert_eq!(spec.cwd.as_deref(), Some(workspace.root()));
            assert_eq!(
                spec.args,
                vec!["--session-dir", &session_dir, "--no-approve"]
            );
            assert_native_resource_discovery(&spec);
        }

        let trusted = profile
            .process_spec_with_trust(&workspace, WorkspaceTrustState::Trusted)
            .unwrap();
        assert_eq!(
            trusted.args,
            vec!["--session-dir", &session_dir, "--approve"]
        );
        assert_native_resource_discovery(&trusted);
    }

    fn assert_native_resource_discovery(spec: &ProcessSpec) {
        assert!(spec.args.iter().any(|arg| arg == "--session-dir"));
        assert!(!spec.env.contains_key("PI_CODING_AGENT_DIR"));
        for blocked in [
            "--no-extensions",
            "--no-skills",
            "--no-prompt-templates",
            "--no-themes",
            "--no-context-files",
        ] {
            assert!(!spec.args.iter().any(|arg| arg == blocked));
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_directory_session_target() {
        let temp = TempDir::new().unwrap();
        let profile = NativePiProfile::new(temp.path().join("state")).unwrap();
        let workspace_path = temp.path().join("workspace");
        fs::create_dir(&workspace_path).unwrap();
        let workspace = Workspace::discover(workspace_path).unwrap();
        let session = profile.session_dir(&workspace).unwrap();
        fs::create_dir_all(session.parent().unwrap()).unwrap();
        fs::write(&session, b"not a directory").unwrap();

        assert!(
            profile
                .process_spec_with_trust(&workspace, WorkspaceTrustState::Untrusted)
                .unwrap_err()
                .to_string()
                .contains("unsafe file type")
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_state_path_is_rejected_without_lossy_rewrite() {
        use std::os::unix::ffi::OsStringExt;

        let temp = TempDir::new().unwrap();
        let mut bytes = temp.path().as_os_str().as_encoded_bytes().to_vec();
        bytes.extend_from_slice(b"/state-");
        bytes.push(0xff);
        let state = PathBuf::from(std::ffi::OsString::from_vec(bytes));
        assert!(
            NativePiProfile::new(state)
                .unwrap_err()
                .to_string()
                .contains("not valid UTF-8")
        );
    }
}
