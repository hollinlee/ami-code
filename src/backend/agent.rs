use super::{BackendKind, BackendSpec, process_spec};
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
        process_spec("pi", self.display_name(), workspace)
    }
}
