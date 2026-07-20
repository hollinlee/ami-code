use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

const MENU_WIDTH: u16 = 12;
const MENU_HEIGHT: u16 = 4;
const MENU_BACKGROUND: Color = Color::Rgb(38, 38, 40);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMenuAction {
    Copy,
    Paste,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextMenu {
    area: Rect,
    copy_enabled: bool,
    hovered: Option<ContextMenuAction>,
}

impl ContextMenu {
    pub fn new(viewport: Rect, column: u16, row: u16, copy_enabled: bool) -> Self {
        let width = MENU_WIDTH.min(viewport.width);
        let height = MENU_HEIGHT.min(viewport.height);
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
            hovered: None,
        }
    }

    pub fn update_hover(&mut self, column: u16, row: u16) {
        self.hovered = self.enabled_action_at(column, row);
    }

    pub fn action_at(self, column: u16, row: u16) -> Option<ContextMenuAction> {
        self.enabled_action_at(column, row)
    }

    fn enabled_action_at(self, column: u16, row: u16) -> Option<ContextMenuAction> {
        match self.command_at(column, row) {
            Some(ContextMenuAction::Copy) if !self.copy_enabled => None,
            action => action,
        }
    }

    fn command_at(self, column: u16, row: u16) -> Option<ContextMenuAction> {
        if column <= self.area.x
            || column >= self.area.right().saturating_sub(1)
            || row <= self.area.y
            || row >= self.area.bottom().saturating_sub(1)
        {
            return None;
        }
        match row - self.area.y - 1 {
            0 => Some(ContextMenuAction::Copy),
            1 => Some(ContextMenuAction::Paste),
            _ => None,
        }
    }

    #[cfg(test)]
    fn area(self) -> Rect {
        self.area
    }

    #[cfg(test)]
    fn hovered(self) -> Option<ContextMenuAction> {
        self.hovered
    }
}

pub fn render_context_menu(frame: &mut ratatui::Frame<'_>, menu: ContextMenu) {
    let copy_style = menu_item_style(
        menu.hovered == Some(ContextMenuAction::Copy),
        menu.copy_enabled,
    );
    let paste_style = menu_item_style(menu.hovered == Some(ContextMenuAction::Paste), true);
    let lines = vec![
        Line::styled(" Copy     ", copy_style),
        Line::styled(" Paste    ", paste_style),
    ];
    frame.render_widget(Clear, menu.area);
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(MENU_BACKGROUND))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray)),
            ),
        menu.area,
    );
}

fn menu_item_style(hovered: bool, enabled: bool) -> Style {
    if hovered && enabled {
        Style::default()
            .fg(Color::Black)
            .bg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else if enabled {
        Style::default().fg(Color::White).bg(MENU_BACKGROUND)
    } else {
        Style::default().fg(Color::DarkGray).bg(MENU_BACKGROUND)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_clamps_to_viewport_and_owns_only_command_cells() {
        let menu = ContextMenu::new(Rect::new(5, 3, 20, 10), 24, 12, true);
        assert_eq!(menu.area(), Rect::new(13, 9, 12, 4));
        assert_eq!(menu.action_at(14, 10), Some(ContextMenuAction::Copy));
        assert_eq!(menu.action_at(14, 11), Some(ContextMenuAction::Paste));
        assert_eq!(menu.action_at(13, 10), None);
        assert_eq!(menu.action_at(14, 9), None);
    }

    #[test]
    fn fixed_menu_disables_copy_without_changing_content_geometry() {
        let mut menu = ContextMenu::new(Rect::new(0, 0, 20, 10), 2, 2, false);
        assert_eq!(menu.area().height, MENU_HEIGHT);
        assert_eq!(menu.action_at(3, 3), None);
        assert_eq!(menu.action_at(3, 4), Some(ContextMenuAction::Paste));

        menu.update_hover(3, 3);
        assert_eq!(menu.hovered(), None);
        menu.update_hover(3, 4);
        assert_eq!(menu.hovered(), Some(ContextMenuAction::Paste));
        menu.update_hover(19, 9);
        assert_eq!(menu.hovered(), None);
    }

    #[test]
    fn hover_follows_enabled_copy_and_paste_rows() {
        let mut menu = ContextMenu::new(Rect::new(0, 0, 20, 10), 2, 2, true);
        assert_eq!(menu.hovered(), None);
        menu.update_hover(3, 3);
        assert_eq!(menu.hovered(), Some(ContextMenuAction::Copy));
        menu.update_hover(3, 4);
        assert_eq!(menu.hovered(), Some(ContextMenuAction::Paste));
    }
}
