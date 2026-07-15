use ratatui::layout::Rect;

use super::{PaneId, WorkbenchLayout};

/// Layout chrome has priority over pane borders/content. This makes every cell
/// have one owner and prevents resize gestures from reaching a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutDivider {
    SidebarMain,
    EditorAgent,
    EditorBottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutHandle {
    Sidebar,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseTarget {
    Handle(LayoutHandle),
    Divider(LayoutDivider),
    Sidebar,
    Border(PaneId),
    Content { pane: PaneId, row: u16, col: u16 },
}

impl MouseTarget {
    pub fn pane(self) -> Option<PaneId> {
        match self {
            Self::Sidebar => Some(PaneId::Sidebar),
            Self::Border(pane) | Self::Content { pane, .. } => Some(pane),
            Self::Handle(_) | Self::Divider(_) => None,
        }
    }
}

pub fn layout_handle_position(layout: WorkbenchLayout, handle: LayoutHandle) -> (u16, u16) {
    match handle {
        LayoutHandle::Sidebar if layout.sidebar.width > 0 => {
            (layout.sidebar.x.saturating_add(1), layout.sidebar.y)
        }
        LayoutHandle::Sidebar => (layout.editor.x, layout.editor.y),
        LayoutHandle::Bottom if layout.bottom.height > 0 => {
            (layout.bottom.x.saturating_add(1), layout.bottom.y)
        }
        LayoutHandle::Bottom => (
            layout.editor.x.saturating_add(layout.editor.width / 2),
            layout.editor.bottom().saturating_sub(1),
        ),
    }
}

pub fn hit_test(layout: WorkbenchLayout, column: u16, row: u16) -> Option<MouseTarget> {
    if layout.compact {
        return None;
    }

    // Handles are tested before dividers so their click target remains exact.
    for handle in [LayoutHandle::Sidebar, LayoutHandle::Bottom] {
        let (handle_x, handle_y) = layout_handle_position(layout, handle);
        if column == handle_x && row == handle_y {
            return Some(MouseTarget::Handle(handle));
        }
    }

    if layout.sidebar.width > 0
        && column == layout.editor.x
        && row >= layout.editor.y
        && row < layout.agent.bottom()
    {
        return Some(MouseTarget::Divider(LayoutDivider::SidebarMain));
    }
    if column == layout.agent.x && row >= layout.agent.y && row < layout.agent.bottom() {
        return Some(MouseTarget::Divider(LayoutDivider::EditorAgent));
    }
    if layout.bottom.height > 0
        && row == layout.bottom.y
        && column >= layout.editor.x
        && column < layout.editor.right()
    {
        return Some(MouseTarget::Divider(LayoutDivider::EditorBottom));
    }

    if contains(layout.sidebar, column, row) {
        return Some(MouseTarget::Sidebar);
    }

    for (pane, area) in [
        (PaneId::Editor, layout.editor),
        (PaneId::Agent, layout.agent),
        (PaneId::Bottom, layout.bottom),
    ] {
        if !contains(area, column, row) {
            continue;
        }
        if is_border(area, column, row) {
            return Some(MouseTarget::Border(pane));
        }
        return Some(MouseTarget::Content {
            pane,
            row: row - area.y - 1,
            col: column - area.x - 1,
        });
    }

    None
}

fn contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x && column < area.right() && row >= area.y && row < area.bottom()
}

fn is_border(area: Rect, column: u16, row: u16) -> bool {
    column == area.x
        || column == area.right().saturating_sub(1)
        || row == area.y
        || row == area.bottom().saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench::WorkbenchLayoutConfig;

    fn layout() -> WorkbenchLayout {
        WorkbenchLayout::calculate(Rect::new(0, 0, 120, 40), WorkbenchLayoutConfig::default())
    }

    #[test]
    fn maps_terminal_content_to_local_coordinates() {
        assert_eq!(
            hit_test(layout(), 25, 1),
            Some(MouseTarget::Content {
                pane: PaneId::Editor,
                row: 0,
                col: 0
            })
        );
    }

    #[test]
    fn chrome_has_priority_and_handles_have_priority_over_dividers() {
        let layout = layout();
        assert_eq!(
            hit_test(layout, layout.sidebar.x + 1, layout.sidebar.y),
            Some(MouseTarget::Handle(LayoutHandle::Sidebar))
        );
        assert_eq!(
            hit_test(layout, layout.bottom.x + 1, layout.bottom.y),
            Some(MouseTarget::Handle(LayoutHandle::Bottom))
        );
        assert_eq!(
            hit_test(layout, layout.editor.x, 2),
            Some(MouseTarget::Divider(LayoutDivider::SidebarMain))
        );
        assert_eq!(
            hit_test(layout, layout.agent.x, 2),
            Some(MouseTarget::Divider(LayoutDivider::EditorAgent))
        );
        assert_eq!(
            hit_test(layout, layout.editor.x + 2, layout.bottom.y),
            Some(MouseTarget::Divider(LayoutDivider::EditorBottom))
        );
    }

    #[test]
    fn hidden_panes_keep_a_restore_handle() {
        let layout = WorkbenchLayout::calculate_visible(
            Rect::new(0, 0, 120, 40),
            WorkbenchLayoutConfig::default(),
            super::super::WorkbenchVisibility {
                sidebar: false,
                bottom: false,
            },
        );
        for handle in [LayoutHandle::Sidebar, LayoutHandle::Bottom] {
            let (column, row) = layout_handle_position(layout, handle);
            assert_eq!(
                hit_test(layout, column, row),
                Some(MouseTarget::Handle(handle))
            );
        }
    }
}
