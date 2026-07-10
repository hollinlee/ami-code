use super::{BackendKind, BackendSpec, process_spec};
use crate::terminal::ProcessSpec;
use crate::workspace::Workspace;

#[derive(Debug, Clone, Copy, Default)]
pub struct NvimBackend;

impl BackendSpec for NvimBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Editor
    }

    fn display_name(&self) -> &str {
        "nvim"
    }

    fn process_spec(&self, workspace: &Workspace) -> ProcessSpec {
        process_spec("nvim", self.display_name(), workspace)
    }
}
