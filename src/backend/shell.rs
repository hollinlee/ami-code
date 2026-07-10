use super::{BackendKind, BackendSpec, process_spec};
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
        process_spec(&self.program, self.display_name(), workspace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_spec_uses_workspace_root() {
        let workspace = Workspace::discover(std::env::current_dir().unwrap()).unwrap();
        let spec = ShellBackend::system_default().process_spec(&workspace);

        assert_eq!(spec.cwd.as_deref(), Some(workspace.root()));
        assert_eq!(spec.display_name, "shell");
        assert_eq!(
            spec.env.get("TERM").map(String::as_str),
            Some("xterm-256color")
        );
    }
}
