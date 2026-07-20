use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

const MENU_WIDTH: u16 = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMenuAction {
    Copy,
    Paste,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextMenu {
    area: Rect,
    copy_enabled: bool,
}

impl ContextMenu {
    pub fn new(viewport: Rect, column: u16, row: u16, copy_enabled: bool) -> Self {
        let height = if copy_enabled { 4 } else { 3 };
        let width = MENU_WIDTH.min(viewport.width);
        let height = height.min(viewport.height);
        let max_x = viewport.right().saturating_sub(width);
        let max_y = viewport.bottom().saturating_sub(height);
        Self {
            area: Rect::new(
                column.min(max_x).max(viewport.x),
                row.min(max_y).max(viewport.y),
                width,
                height,
            ),
            copy_enabled,
        }
    }

    pub fn action_at(self, column: u16, row: u16) -> Option<ContextMenuAction> {
        if column <= self.area.x
            || column >= self.area.right().saturating_sub(1)
            || row <= self.area.y
            || row >= self.area.bottom().saturating_sub(1)
        {
            return None;
        }
        let command_row = row - self.area.y - 1;
        match (self.copy_enabled, command_row) {
            (true, 0) => Some(ContextMenuAction::Copy),
            (true, 1) | (false, 0) => Some(ContextMenuAction::Paste),
            _ => None,
        }
    }

    #[cfg(test)]
    fn area(self) -> Rect {
        self.area
    }
}

pub fn render_context_menu(frame: &mut ratatui::Frame<'_>, menu: ContextMenu) {
    let lines = if menu.copy_enabled {
        vec![Line::from("Copy"), Line::from("Paste")]
    } else {
        vec![Line::from("Paste")]
    };
    frame.render_widget(Clear, menu.area);
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(Color::White).bg(Color::DarkGray))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Gray)),
            ),
        menu.area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_clamps_to_viewport_and_owns_only_command_cells() {
        let menu = ContextMenu::new(Rect::new(5, 3, 20, 10), 24, 12, true);
        assert_eq!(menu.area(), Rect::new(16, 9, 9, 4));
        assert_eq!(menu.action_at(17, 10), Some(ContextMenuAction::Copy));
        assert_eq!(menu.action_at(17, 11), Some(ContextMenuAction::Paste));
        assert_eq!(menu.action_at(16, 10), None);
        assert_eq!(menu.action_at(17, 9), None);
    }

    #[test]
    fn paste_only_menu_has_one_command() {
        let menu = ContextMenu::new(Rect::new(0, 0, 20, 10), 2, 2, false);
        assert_eq!(menu.action_at(3, 3), Some(ContextMenuAction::Paste));
        assert_eq!(menu.action_at(3, 4), None);
    }
}
