use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidebarStyle {
    pub focused_border: Color,
    pub unfocused_border: Color,
}

impl Default for SidebarStyle {
    fn default() -> Self {
        Self {
            focused_border: Color::LightYellow,
            unfocused_border: Color::DarkGray,
        }
    }
}

pub fn render_sidebar(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    focused: bool,
    style: SidebarStyle,
) {
    let text = "dummy sidebar";
    let block = Block::default()
        .title("sidebar")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused {
            style.focused_border
        } else {
            style.unfocused_border
        }));

    frame.render_widget(Paragraph::new(text).block(block), area);
}
