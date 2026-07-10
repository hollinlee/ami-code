use super::{BackendKind, BackendSpec, build_backend_process_spec};
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
        build_backend_process_spec(self.display_name(), self.display_name(), workspace)
    }
}
