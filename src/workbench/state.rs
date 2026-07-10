use super::{Direction, FocusGraph, Mode, PaneId, PaneKind, PaneState};
use crate::backend::BackendKind;

#[derive(Debug)]
pub struct WorkbenchState {
    mode: Mode,
    focus_graph: FocusGraph,
    focused_pane: PaneId,
    panes: [PaneState; 4],
}

impl Default for WorkbenchState {
    fn default() -> Self {
        Self {
            mode: Mode::Edit,
            focus_graph: FocusGraph,
            focused_pane: PaneId::Editor,
            panes: [
                PaneState::new(PaneId::Sidebar, PaneKind::Sidebar),
                PaneState::new(PaneId::Editor, PaneKind::Backend(BackendKind::Editor)),
                PaneState::new(PaneId::Agent, PaneKind::Backend(BackendKind::Agent)),
                PaneState::new(PaneId::Bottom, PaneKind::Backend(BackendKind::Shell)),
            ],
        }
    }
}

impl WorkbenchState {
    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    pub fn toggle_control_mode(&mut self) {
        self.mode = match self.mode {
            Mode::Control => Mode::Edit,
            Mode::Edit | Mode::View => Mode::Control,
        };
    }

    pub fn focused_pane(&self) -> PaneId {
        self.focused_pane
    }

    pub fn is_focused(&self, pane: PaneId) -> bool {
        self.focused_pane == pane
    }

    pub fn focus(&mut self, direction: Direction) {
        let target = self.focus_graph.next(self.focused_pane, direction);
        if self.pane(target).is_some_and(|pane| pane.visible) {
            self.focused_pane = target;
        }
    }

    pub fn pane(&self, id: PaneId) -> Option<&PaneState> {
        self.panes.iter().find(|pane| pane.id == id)
    }

    pub fn panes(&self) -> &[PaneState] {
        &self.panes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_editor_in_edit_mode() {
        let state = WorkbenchState::default();

        assert_eq!(state.mode(), Mode::Edit);
        assert_eq!(state.focused_pane(), PaneId::Editor);
        assert_eq!(state.panes().len(), 4);
    }

    #[test]
    fn moves_focus_through_graph() {
        let mut state = WorkbenchState::default();

        state.focus(Direction::Right);
        assert_eq!(state.focused_pane(), PaneId::Agent);
        state.focus(Direction::Left);
        state.focus(Direction::Down);
        assert_eq!(state.focused_pane(), PaneId::Bottom);
        state.focus(Direction::Up);
        state.focus(Direction::Left);
        assert_eq!(state.focused_pane(), PaneId::Sidebar);
    }
}
