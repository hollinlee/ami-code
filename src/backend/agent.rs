use super::{BackendKind, BackendSpec, build_backend_process_spec};
use crate::terminal::ProcessSpec;
use crate::workspace::Workspace;

#[derive(Debug, Clone, Copy, Default)]
pub struct PiBackend;

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
