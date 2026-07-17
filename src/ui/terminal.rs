use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::terminal::{TerminalPoint, TerminalRange, TerminalSize};
use crate::workbench::{ShellTabs, shell_tab_geometry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalPaneStyle {
    pub focused_border: Color,
    pub unfocused_border: Color,
}

impl Default for TerminalPaneStyle {
    fn default() -> Self {
        Self {
            focused_border: Color::LightYellow,
            unfocused_border: Color::Cyan,
        }
    }
}

pub fn terminal_content_size(area: Rect) -> TerminalSize {
    TerminalSize::new(area.width.saturating_sub(2), area.height.saturating_sub(2))
}

pub fn shell_terminal_content_size(area: Rect) -> TerminalSize {
    TerminalSize::new(area.width.saturating_sub(2), area.height.saturating_sub(3))
}

pub fn render_compact_workbench(frame: &mut ratatui::Frame<'_>, area: Rect) {
    // This view deliberately does not inspect a terminal screen. Sessions remain
    // alive at their last valid size until enough room is available again.
    frame.render_widget(
        Paragraph::new("workbench needs at least 6 columns × 3 rows")
            .style(Style::default().fg(Color::Yellow)),
        area,
    );
}

pub fn render_unavailable_terminal_pane(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    message: &str,
    focused: bool,
    pane_style: TerminalPaneStyle,
) {
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused {
            pane_style.focused_border
        } else {
            pane_style.unfocused_border
        }));
    frame.render_widget(
        Paragraph::new(message)
            .style(Style::default().fg(Color::Yellow))
            .block(block),
        area,
    );
}

pub struct ShellTerminalPaneView<'a> {
    pub screen: Option<&'a vt100::Screen>,
    pub title: &'a str,
    pub message: Option<&'a str>,
    pub focused: bool,
    pub selection: Option<TerminalRange>,
    pub tabs: &'a ShellTabs,
    pub style: TerminalPaneStyle,
}

pub fn render_shell_terminal_pane(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    view: ShellTerminalPaneView<'_>,
) {
    let block = Block::default()
        .title(view.title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if view.focused {
            view.style.focused_border
        } else {
            view.style.unfocused_border
        }));
    frame.render_widget(block, area);

    let (geometry, plus) = shell_tab_geometry(area, view.tabs);
    for tab in geometry {
        let active = tab.id == view.tabs.active();
        let label = tab_label(tab.display_number, tab.width, active);
        let style = if active {
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightCyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        frame.render_widget(
            Paragraph::new(label).style(style),
            Rect::new(tab.x, area.y + 1, tab.width, 1),
        );
    }
    if let Some((x, y)) = plus {
        frame.render_widget(
            Paragraph::new("+").style(Style::default().fg(Color::LightGreen)),
            Rect::new(x, y, 1, 1),
        );
    }

    let content = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(2),
        area.width.saturating_sub(2),
        area.height.saturating_sub(3),
    );
    if let Some(screen) = view.screen {
        frame.render_widget(
            Paragraph::new(styled_screen_lines(screen, view.selection)),
            content,
        );
    } else {
        frame.render_widget(
            Paragraph::new(view.message.unwrap_or("unavailable"))
                .style(Style::default().fg(Color::Yellow)),
            content,
        );
    }
}

fn tab_label(display_number: usize, width: u16, active: bool) -> String {
    let marker = if active { "●" } else { " " };
    let raw = format!("{marker}{display_number} ×");
    let mut chars: String = raw.chars().take(width as usize).collect();
    while chars.chars().count() < width as usize {
        chars.push(' ');
    }
    // Geometry assigns the final cell to close regardless of truncation.
    if width > 0 {
        chars.pop();
        chars.push('×');
    }
    chars
}

pub fn render_terminal_pane(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    screen: &vt100::Screen,
    title: &str,
    focused: bool,
    selection: Option<TerminalRange>,
    pane_style: TerminalPaneStyle,
) {
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused {
            pane_style.focused_border
        } else {
            pane_style.unfocused_border
        }));

    frame.render_widget(
        Paragraph::new(styled_screen_lines(screen, selection)).block(block),
        area,
    );
}

fn styled_screen_lines(
    screen: &vt100::Screen,
    selection: Option<TerminalRange>,
) -> Vec<Line<'static>> {
    let (rows, cols) = screen.size();
    let cursor = screen.cursor_position();
    let scrollback = i64::try_from(screen.scrollback()).unwrap_or(i64::MAX);
    let mut lines = Vec::with_capacity(rows as usize);

    for row in 0..rows {
        let mut spans = Vec::with_capacity(cols as usize);
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                spans.push(Span::raw(" "));
                continue;
            };

            if cell.is_wide_continuation() {
                continue;
            }

            let contents = if cell.has_contents() {
                cell.contents().to_string()
            } else {
                " ".to_string()
            };
            let point = TerminalPoint::new(i64::from(row) - scrollback, col);
            spans.push(Span::styled(
                contents,
                cell_style(
                    cell,
                    cursor == (row, col),
                    selection.is_some_and(|selection| selection.contains(point)),
                ),
            ));
        }
        lines.push(Line::from(spans));
    }

    lines
}

fn cell_style(cell: &vt100::Cell, is_cursor: bool, is_selected: bool) -> Style {
    let mut foreground = color_to_ratatui(cell.fgcolor());
    let mut background = color_to_ratatui(cell.bgcolor());

    if cell.inverse() {
        invert_colors(&mut foreground, &mut background, Color::White);
    }

    if is_cursor {
        invert_colors(&mut foreground, &mut background, Color::LightYellow);
    }
    if is_selected {
        foreground = Some(Color::Black);
        background = Some(Color::LightBlue);
    }

    let mut style = Style::default();
    if let Some(color) = foreground {
        style = style.fg(color);
    }
    if let Some(color) = background {
        style = style.bg(color);
    }
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.dim() {
        style = style.add_modifier(Modifier::DIM);
    }

    style
}

fn invert_colors(foreground: &mut Option<Color>, background: &mut Option<Color>, fallback: Color) {
    match (*foreground, *background) {
        (None, None) => {
            *foreground = Some(Color::Black);
            *background = Some(fallback);
        }
        _ => std::mem::swap(foreground, background),
    }
}

fn color_to_ratatui(color: vt100::Color) -> Option<Color> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(index) => Some(indexed_color(index)),
        vt100::Color::Rgb(red, green, blue) => Some(Color::Rgb(red, green, blue)),
    }
}

fn indexed_color(index: u8) -> Color {
    match index {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::Gray,
        8 => Color::DarkGray,
        9 => Color::LightRed,
        10 => Color::LightGreen,
        11 => Color::LightYellow,
        12 => Color::LightBlue,
        13 => Color::LightMagenta,
        14 => Color::LightCyan,
        15 => Color::White,
        value => Color::Indexed(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_size_excludes_border() {
        assert_eq!(
            terminal_content_size(Rect::new(0, 0, 120, 30)),
            TerminalSize::new(118, 28)
        );
        assert_eq!(
            terminal_content_size(Rect::new(0, 0, 1, 1)),
            TerminalSize::new(2, 2)
        );
        assert_eq!(
            shell_terminal_content_size(Rect::new(0, 0, 120, 30)),
            TerminalSize::new(118, 27)
        );
        assert_eq!(
            shell_terminal_content_size(Rect::new(0, 0, 20, 5)),
            TerminalSize::new(18, 2)
        );
    }

    #[test]
    fn preserves_cell_colors_and_inverse() {
        let mut parser = vt100::Parser::new(1, 2, 0);
        parser.process(b"\x1b[31;44;7mA");
        let lines = styled_screen_lines(parser.screen(), None);
        let style = lines[0].spans[0].style;

        assert_eq!(style.fg, Some(Color::Blue));
        assert_eq!(style.bg, Some(Color::Red));
    }

    #[test]
    fn preserves_rgb_colors_and_modifiers() {
        let mut parser = vt100::Parser::new(1, 2, 0);
        parser.process(b"\x1b[38;2;1;2;3;48;2;4;5;6;1;3;4mA");
        let lines = styled_screen_lines(parser.screen(), None);
        let style = lines[0].spans[0].style;

        assert_eq!(style.fg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(style.bg, Some(Color::Rgb(4, 5, 6)));
        assert!(style.add_modifier.contains(Modifier::BOLD));
        assert!(style.add_modifier.contains(Modifier::ITALIC));
        assert!(style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn renders_cursor_as_highlighted_cell() {
        let parser = vt100::Parser::new(1, 1, 0);
        let lines = styled_screen_lines(parser.screen(), None);
        let style = lines[0].spans[0].style;

        assert_eq!(style.fg, Some(Color::Black));
        assert_eq!(style.bg, Some(Color::LightYellow));
    }

    #[test]
    fn highlights_selected_cells() {
        let mut parser = vt100::Parser::new(1, 3, 0);
        parser.process(b"abc");
        let selection =
            TerminalRange::inclusive(TerminalPoint::new(0, 1), TerminalPoint::new(0, 2));
        let lines = styled_screen_lines(parser.screen(), Some(selection));

        assert_ne!(lines[0].spans[0].style.bg, Some(Color::LightBlue));
        assert_eq!(lines[0].spans[1].style.bg, Some(Color::LightBlue));
        assert_eq!(lines[0].spans[2].style.bg, Some(Color::LightBlue));
    }
}
