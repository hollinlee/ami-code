mod focus;
mod mode;
mod pane;

pub use focus::FocusGraph;
pub use mode::Mode;
pub use pane::{PaneId, PaneKind, PaneState};

#[derive(Debug)]
pub struct Workbench {
    mode: Mode,
    focus_graph: FocusGraph,
    focused_pane: PaneId,
}

impl Workbench {
    pub fn new(focus_graph: FocusGraph) -> Self {
        Self {
            mode: Mode::Edit,
            focus_graph,
            focused_pane: PaneId::Editor,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn focused_pane(&self) -> PaneId {
        self.focused_pane
    }

    pub fn focus_graph(&self) -> &FocusGraph {
        &self.focus_graph
    }
}
