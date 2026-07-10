use crate::backend::BackendKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaneId {
    Sidebar,
    Editor,
    Agent,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneKind {
    Sidebar,
    Backend(BackendKind),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneState {
    pub id: PaneId,
    pub kind: PaneKind,
    pub focused: bool,
    pub visible: bool,
}

impl PaneState {
    pub fn new(id: PaneId, kind: PaneKind) -> Self {
        Self {
            id,
            kind,
            focused: false,
            visible: true,
        }
    }
}
