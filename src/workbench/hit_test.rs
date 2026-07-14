use ratatui::layout::Rect;

use super::{PaneId, WorkbenchLayout};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseTarget {
    Sidebar,
    Border(PaneId),
    Content { pane: PaneId, row: u16, col: u16 },
}

impl MouseTarget {
    pub fn pane(self) -> PaneId {
        match self {
            Self::Sidebar => PaneId::Sidebar,
            Self::Border(pane) | Self::Content { pane, .. } => pane,
        }
    }
}

pub fn hit_test(layout: WorkbenchLayout, column: u16, row: u16) -> Option<MouseTarget> {
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
                col: 0,
            })
        );
        assert_eq!(
            hit_test(layout(), 81, 1),
            Some(MouseTarget::Content {
                pane: PaneId::Agent,
                row: 0,
                col: 0,
            })
        );
    }

    #[test]
    fn distinguishes_sidebar_and_terminal_borders() {
        assert_eq!(hit_test(layout(), 2, 2), Some(MouseTarget::Sidebar));
        assert_eq!(
            hit_test(layout(), 24, 0),
            Some(MouseTarget::Border(PaneId::Editor))
        );
        assert_eq!(
            hit_test(layout(), 79, 39),
            Some(MouseTarget::Border(PaneId::Bottom))
        );
    }

    #[test]
    fn rejects_coordinates_outside_layout() {
        assert_eq!(hit_test(layout(), 120, 40), None);
    }
}
