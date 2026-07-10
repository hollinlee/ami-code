use crate::workbench::{FocusGraph, Mode, PaneId, Workbench};
use crate::workspace::Workspace;

#[derive(Debug)]
pub struct App {
    workspace: Workspace,
    workbench: Workbench,
}

impl App {
    pub fn new(workspace: Workspace) -> Self {
        Self {
            workspace,
            workbench: Workbench::new(FocusGraph::default()),
        }
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn mode(&self) -> Mode {
        self.workbench.mode()
    }

    pub fn focused_pane(&self) -> PaneId {
        self.workbench.focused_pane()
    }
}
