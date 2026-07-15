mod sidebar;
mod terminal;

use crate::workbench::WorkbenchLayout;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Paragraph;

pub use sidebar::{SidebarStyle, render_sidebar};
pub use terminal::{
    TerminalPaneStyle, render_compact_workbench, render_terminal_pane,
    render_unavailable_terminal_pane, terminal_content_size,
};

/// Paint compact, familiar restore/collapse affordances over pane borders.
pub fn render_layout_controls(frame: &mut ratatui::Frame<'_>, layout: WorkbenchLayout) {
    if layout.compact {
        return;
    }
    let style = Style::default().fg(Color::DarkGray);
    let sidebar_symbol = if layout.sidebar.width > 0 {
        "◀"
    } else {
        "▶"
    };
    frame.render_widget(
        Paragraph::new(sidebar_symbol).style(style),
        Rect::new(layout.editor.x, layout.editor.y, 1, 1),
    );

    let bottom_y = if layout.bottom.height > 0 {
        layout.bottom.y
    } else {
        layout.editor.bottom().saturating_sub(1)
    };
    let bottom_symbol = if layout.bottom.height > 0 {
        "▼"
    } else {
        "▲"
    };
    frame.render_widget(
        Paragraph::new(bottom_symbol).style(style),
        Rect::new(
            layout.editor.x.saturating_add(layout.editor.width / 2),
            bottom_y,
            1,
            1,
        ),
    );
}
