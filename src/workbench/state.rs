use super::{Direction, FocusGraph, Mode, PaneId, PaneKind, PaneSelection, PaneState};
use crate::backend::BackendKind;
use crate::terminal::{TerminalPoint, TerminalRange};

#[derive(Debug)]
pub struct WorkbenchState {
    mode: Mode,
    focus_graph: FocusGraph,
    focused_pane: PaneId,
    panes: [PaneState; 4],
    selection: Option<PaneSelection>,
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
            selection: None,
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

    pub fn focus(&mut self, direction: Direction) -> bool {
        let target = self.focus_graph.next(self.focused_pane, direction);
        self.focus_pane(target)
    }

    pub fn focus_pane(&mut self, target: PaneId) -> bool {
        if target != self.focused_pane && self.pane(target).is_some_and(|pane| pane.visible) {
            self.focused_pane = target;
            true
        } else {
            false
        }
    }

    pub fn begin_selection(&mut self, anchor: TerminalPoint) {
        self.selection = Some(PaneSelection::new(self.focused_pane, anchor));
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    pub fn selection(&self) -> Option<PaneSelection> {
        self.selection
    }

    pub fn selection_range(&self, pane: PaneId) -> Option<TerminalRange> {
        self.selection
            .filter(|selection| selection.pane() == pane)
            .map(PaneSelection::range)
    }

    pub fn set_selection_head(&mut self, head: TerminalPoint) {
        if let Some(selection) = &mut self.selection {
            selection.set_head(head);
        }
    }

    pub fn pane(&self, id: PaneId) -> Option<&PaneState> {
        self.panes.iter().find(|pane| pane.id == id)
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
        assert!(state.pane(PaneId::Sidebar).is_some());
        assert!(state.pane(PaneId::Editor).is_some());
        assert!(state.pane(PaneId::Agent).is_some());
        assert!(state.pane(PaneId::Bottom).is_some());
    }

    #[test]
    fn owns_selection_for_one_pane() {
        let mut state = WorkbenchState::default();
        state.begin_selection(TerminalPoint::new(2, 3));
        state.set_selection_head(TerminalPoint::new(4, 5));

        assert_eq!(
            state.selection_range(PaneId::Editor),
            Some(TerminalRange::inclusive(
                TerminalPoint::new(2, 3),
                TerminalPoint::new(4, 5),
            ))
        );
        assert_eq!(state.selection_range(PaneId::Agent), None);

        state.clear_selection();
        assert_eq!(state.selection(), None);
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
