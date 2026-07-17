use super::{BackendKind, BackendSpec, build_backend_process_spec};
use crate::terminal::ProcessSpec;
use crate::workspace::Workspace;

#[derive(Debug, Clone)]
pub struct ShellBackend {
    program: String,
}

impl ShellBackend {
    pub fn system_default() -> Self {
        Self {
            program: std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string()),
        }
    }
}

impl BackendSpec for ShellBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Shell
    }
    fn display_name(&self) -> &str {
        "shell"
    }

    fn process_spec(&self, workspace: &Workspace) -> ProcessSpec {
        let chosen_shell =
            build_backend_process_spec(&self.program, self.display_name(), workspace);
        // The selected shell is a positional argument to fixed source, never
        // interpolated into it. `stty` therefore disables IXON on the embedded
        // PTY without turning workspace or user values into commands.
        ProcessSpec {
            program: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                "stty -ixon < /dev/tty 2>/dev/null || true; exec \"$1\"".to_string(),
                "ami-code-shell-wrapper".to_string(),
                self.program.clone(),
            ],
            env: chosen_shell.env,
            cwd: chosen_shell.cwd,
            display_name: chosen_shell.display_name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_spec_uses_workspace_root_and_ixon_wrapper() {
        let workspace = Workspace::discover(std::env::current_dir().unwrap()).unwrap();
        let backend = ShellBackend::system_default();
        let spec = backend.process_spec(&workspace);
        assert_eq!(spec.cwd.as_deref(), Some(workspace.root()));
        assert_eq!(spec.display_name, "shell");
        assert_eq!(
            spec.env.get("TERM").map(String::as_str),
            Some("xterm-256color")
        );
        assert_eq!(spec.program, "/bin/sh");
        assert!(spec.args[1].contains("stty -ixon"));
        assert_eq!(spec.args.last(), Some(&backend.program));
    }

    #[test]
    fn chosen_shell_is_a_positional_argument_to_fixed_wrapper() {
        let workspace = Workspace::discover(std::env::current_dir().unwrap()).unwrap();
        let backend = ShellBackend {
            program: "/tmp/a shell; echo unsafe".to_string(),
        };
        let spec = backend.process_spec(&workspace);
        assert_eq!(
            spec.args[1],
            "stty -ixon < /dev/tty 2>/dev/null || true; exec \"$1\""
        );
        assert_eq!(spec.args[3], "/tmp/a shell; echo unsafe");
        assert!(!spec.args[1].contains("unsafe"));
    }
}
