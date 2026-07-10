use ratatui::layout::{Constraint, Direction, Layout, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkbenchLayoutConfig {
    pub sidebar_width: u16,
    pub agent_width: u16,
    pub bottom_height: u16,
}

impl Default for WorkbenchLayoutConfig {
    fn default() -> Self {
        Self {
            sidebar_width: 24,
            agent_width: 40,
            bottom_height: 12,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkbenchLayout {
    pub sidebar: Rect,
    pub editor: Rect,
    pub agent: Rect,
    pub bottom: Rect,
}

impl WorkbenchLayout {
    pub fn calculate(area: Rect, config: WorkbenchLayoutConfig) -> Self {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(config.sidebar_width),
                Constraint::Min(1),
                Constraint::Length(config.agent_width),
            ])
            .split(area);

        let middle = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(config.bottom_height)])
            .split(columns[1]);

        Self {
            sidebar: columns[0],
            editor: middle[0],
            bottom: middle[1],
            agent: columns[2],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_default_workbench_dimensions() {
        let area = Rect::new(0, 0, 160, 50);
        let layout = WorkbenchLayout::calculate(area, WorkbenchLayoutConfig::default());

        assert_eq!(layout.sidebar.width, 24);
        assert_eq!(layout.agent.width, 40);
        assert_eq!(layout.bottom.height, 12);
        assert_eq!(layout.editor.x, 24);
        assert_eq!(layout.agent.x, 120);
        assert_eq!(layout.bottom.y, 38);
    }

    #[test]
    fn keeps_narrow_layout_inside_available_area() {
        let area = Rect::new(0, 0, 40, 10);
        let layout = WorkbenchLayout::calculate(area, WorkbenchLayoutConfig::default());

        for pane in [layout.sidebar, layout.editor, layout.agent, layout.bottom] {
            assert!(pane.x.saturating_add(pane.width) <= area.right());
            assert!(pane.y.saturating_add(pane.height) <= area.bottom());
        }
        assert!(layout.editor.width >= 1);
        assert!(layout.editor.height >= 1);
    }
}
