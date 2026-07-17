mod agent;
mod editor;
mod shell;

pub use agent::PiBackend;
#[allow(unused_imports)]
pub use editor::{
    ManagedNvimGeneration, ManagedNvimProfile, NvimBackend, NvimController, NvimRemoteError,
};
pub use shell::ShellBackend;

use crate::terminal::ProcessSpec;
use crate::workspace::Workspace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Editor,
    Agent,
    Shell,
}

pub trait BackendSpec {
    fn kind(&self) -> BackendKind;
    fn display_name(&self) -> &str;
    fn process_spec(&self, workspace: &Workspace) -> ProcessSpec;
}

fn build_backend_process_spec(
    program: impl Into<String>,
    display_name: impl Into<String>,
    workspace: &Workspace,
) -> ProcessSpec {
    ProcessSpec::new(program)
        .display_name(display_name)
        .env("TERM", "xterm-256color")
        .cwd(workspace.root())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvim_backend_kind_is_editor() {
        let backend = NvimBackend;
        assert_eq!(backend.kind(), BackendKind::Editor);
        assert_eq!(backend.display_name(), "nvim");
    }

    #[test]
    fn pi_spec_targets_workspace() {
        let workspace = Workspace::discover(std::env::current_dir().unwrap()).unwrap();
        let backend = PiBackend;
        let spec = backend.process_spec(&workspace);

        assert_eq!(backend.kind(), BackendKind::Agent);
        assert_eq!(spec.program, "pi");
        assert_eq!(spec.display_name, "pi");
        assert_eq!(spec.cwd.as_deref(), Some(workspace.root()));
        assert_eq!(
            spec.env.get("TERM").map(String::as_str),
            Some("xterm-256color")
        );
    }
}
