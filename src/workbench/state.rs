use ratatui::layout::Rect;

use super::{
    LayoutIntent, PaneId, PaneKind, PaneSelection, PaneState, ShellTabs, WorkbenchLayoutConfig,
    WorkbenchVisibility,
};
use crate::backend::BackendKind;
use crate::terminal::{TerminalPoint, TerminalRange};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CollapseState {
    manual: bool,
    automatic: bool,
}

impl CollapseState {
    pub fn is_collapsed(self) -> bool {
        self.manual || self.automatic
    }

    #[cfg(test)]
    pub fn is_manually_collapsed(self) -> bool {
        self.manual
    }

    #[cfg(test)]
    pub fn is_automatically_collapsed(self) -> bool {
        self.automatic
    }

    fn toggle_manual(&mut self) {
        self.manual = !self.manual;
    }
}

#[derive(Debug)]
pub struct WorkbenchState {
    focused_pane: PaneId,
    panes: [PaneState; 4],
    selection: Option<PaneSelection>,
    sidebar_collapse: CollapseState,
    bottom_collapse: CollapseState,
    shell_tabs: ShellTabs,
}

impl Default for WorkbenchState {
    fn default() -> Self {
        Self {
            focused_pane: PaneId::Editor,
            panes: [
                PaneState::new(PaneId::Sidebar, PaneKind::Sidebar),
                PaneState::new(PaneId::Editor, PaneKind::Backend(BackendKind::Editor)),
                PaneState::new(PaneId::Agent, PaneKind::Backend(BackendKind::Agent)),
                PaneState::new(PaneId::Bottom, PaneKind::Backend(BackendKind::Shell)),
            ],
            selection: None,
            sidebar_collapse: CollapseState::default(),
            bottom_collapse: CollapseState::default(),
            shell_tabs: ShellTabs::default(),
        }
    }
}

impl WorkbenchState {
    pub fn shell_tabs(&self) -> &ShellTabs {
        &self.shell_tabs
    }

    pub fn shell_tabs_mut(&mut self) -> &mut ShellTabs {
        &mut self.shell_tabs
    }

    pub fn focused_pane(&self) -> PaneId {
        self.focused_pane
    }

    pub fn is_focused(&self, pane: PaneId) -> bool {
        self.focused_pane == pane
    }

    pub fn focus_pane(&mut self, target: PaneId) -> bool {
        if target != self.focused_pane && self.pane(target).is_some_and(|pane| pane.visible) {
            self.focused_pane = target;
            true
        } else {
            false
        }
    }

    pub fn toggle_sidebar(&mut self) {
        self.sidebar_collapse.toggle_manual();
        self.update_pane_visibility();
    }

    pub fn toggle_bottom(&mut self) {
        self.bottom_collapse.toggle_manual();
        self.update_pane_visibility();
    }

    pub fn set_manual_collapse(&mut self, sidebar: bool, bottom: bool) {
        self.sidebar_collapse.manual = sidebar;
        self.bottom_collapse.manual = bottom;
        self.update_pane_visibility();
    }

    pub fn manual_collapse(&self) -> (bool, bool) {
        (self.sidebar_collapse.manual, self.bottom_collapse.manual)
    }

    pub fn layout_intent(&self, config: WorkbenchLayoutConfig) -> LayoutIntent {
        let (sidebar_collapsed, bottom_collapsed) = self.manual_collapse();
        LayoutIntent {
            config,
            sidebar_collapsed,
            bottom_collapsed,
        }
    }

    #[cfg(test)]
    pub fn sidebar_collapse(&self) -> CollapseState {
        self.sidebar_collapse
    }

    #[cfg(test)]
    pub fn bottom_collapse(&self) -> CollapseState {
        self.bottom_collapse
    }

    pub fn update_auto_collapse(&mut self, area: Rect, config: WorkbenchLayoutConfig) {
        self.sidebar_collapse.automatic = area.width < config.sidebar_shortage_width();
        self.bottom_collapse.automatic = area.height < config.bottom_shortage_height();
        self.update_pane_visibility();
    }

    pub fn visibility(&self) -> WorkbenchVisibility {
        WorkbenchVisibility {
            sidebar: !self.sidebar_collapse.is_collapsed(),
            bottom: !self.bottom_collapse.is_collapsed(),
        }
    }

    fn update_pane_visibility(&mut self) {
        let visibility = self.visibility();
        for pane in &mut self.panes {
            pane.visible = match pane.id {
                PaneId::Sidebar => visibility.sidebar,
                PaneId::Bottom => visibility.bottom,
                PaneId::Editor | PaneId::Agent => true,
            };
        }
        if !self
            .pane(self.focused_pane)
            .is_some_and(|pane| pane.visible)
        {
            self.focused_pane = PaneId::Editor;
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
    fn defaults_to_editor_focus() {
        let state = WorkbenchState::default();
        assert_eq!(state.focused_pane(), PaneId::Editor);
        assert!(state.pane(PaneId::Sidebar).is_some());
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
                TerminalPoint::new(4, 5)
            ))
        );
        assert_eq!(state.selection_range(PaneId::Agent), None);
        state.clear_selection();
        assert_eq!(state.selection(), None);
    }

    #[test]
    fn auto_collapse_uses_thresholds_and_recovers() {
        let mut state = WorkbenchState::default();
        let config = WorkbenchLayoutConfig::default();
        state.update_auto_collapse(
            Rect::new(
                0,
                0,
                config.sidebar_shortage_width().saturating_sub(1),
                config.bottom_shortage_height().saturating_sub(1),
            ),
            config,
        );
        assert!(state.sidebar_collapse().is_automatically_collapsed());
        assert!(state.bottom_collapse().is_automatically_collapsed());
        state.update_auto_collapse(
            Rect::new(
                0,
                0,
                config.sidebar_shortage_width(),
                config.bottom_shortage_height(),
            ),
            config,
        );
        assert!(!state.sidebar_collapse().is_collapsed());
        assert!(!state.bottom_collapse().is_collapsed());
    }

    #[test]
    fn automatic_collapse_never_changes_serialized_manual_intent() {
        let mut state = WorkbenchState::default();
        state.toggle_sidebar();
        let config = WorkbenchLayoutConfig::default();
        let before = state.layout_intent(config);
        state.update_auto_collapse(Rect::new(0, 0, 1, 1), config);
        assert_eq!(state.layout_intent(config), before);
    }

    #[test]
    fn manual_and_auto_collapse_follow_full_truth_table() {
        let config = WorkbenchLayoutConfig::default();
        for manual in [false, true] {
            for automatic in [false, true] {
                let mut state = WorkbenchState::default();
                if manual {
                    state.toggle_sidebar();
                }
                let width = if automatic {
                    config.sidebar_shortage_width().saturating_sub(1)
                } else {
                    config.sidebar_shortage_width()
                };
                state.update_auto_collapse(Rect::new(0, 0, width, 24), config);
                assert_eq!(state.sidebar_collapse().is_manually_collapsed(), manual);
                assert_eq!(
                    state.sidebar_collapse().is_automatically_collapsed(),
                    automatic
                );
                assert_eq!(state.sidebar_collapse().is_collapsed(), manual || automatic);
            }
        }
    }

    #[test]
    fn layout_toggle_preserves_unrelated_focus_and_selection() {
        let mut state = WorkbenchState::default();
        state.begin_selection(TerminalPoint::new(2, 3));
        state.set_selection_head(TerminalPoint::new(4, 5));

        state.toggle_sidebar();
        state.toggle_bottom();

        assert_eq!(state.focused_pane(), PaneId::Editor);
        assert_eq!(
            state.selection_range(PaneId::Editor),
            Some(TerminalRange::inclusive(
                TerminalPoint::new(2, 3),
                TerminalPoint::new(4, 5)
            ))
        );
    }

    #[test]
    fn collapsing_focused_pane_returns_focus_to_editor() {
        let mut state = WorkbenchState::default();
        assert!(state.focus_pane(PaneId::Bottom));
        state.toggle_bottom();
        assert_eq!(state.focused_pane(), PaneId::Editor);
    }
}
