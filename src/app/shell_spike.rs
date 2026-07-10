use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::terminal::{ProcessSpec, TerminalSession, TerminalSize};

pub fn run() -> Result<()> {
    run_shell_session()
}

pub fn run_nvim() -> Result<()> {
    run_command(EmbeddedCommand::Nvim)
}

pub fn run_pi() -> Result<()> {
    run_command(EmbeddedCommand::Pi)
}

fn run_shell_session() -> Result<()> {
    let mut terminal_guard = TerminalGuard::enter()?;
    let size = terminal_guard.terminal.size()?;
    let terminal_size = inner_terminal_size_value(size.width, size.height);
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
    let spec = ProcessSpec::new(shell)
        .display_name("shell")
        .env("TERM", "xterm-256color")
        .cwd(std::env::current_dir()?);
    let mut session = TerminalSession::spawn(spec, terminal_size, 1_000)?;

    loop {
        session.poll_output()?;
        if session.has_exited()? {
            break;
        }

        let size = terminal_guard.terminal.size()?;
        session.resize(inner_terminal_size_value(size.width, size.height))?;

        terminal_guard.terminal.draw(|frame| {
            render_pty(
                frame.area(),
                frame,
                session.parser(),
                session.display_name(),
                true,
            );
        })?;

        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) if is_quit(key) => break,
                Event::Key(key) => session.send_key(key)?,
                Event::Resize(cols, rows) => {
                    session.resize(inner_terminal_size_value(cols, rows))?;
                }
                _ => {}
            }
        }
    }

    session.terminate();
    Ok(())
}

fn run_command(command: EmbeddedCommand) -> Result<()> {
    let mut terminal_guard = TerminalGuard::enter()?;
    let size = terminal_guard.terminal.size()?;
    let (pty_cols, pty_rows) = inner_terminal_size(size.width, size.height);

    let mut pty = PtyProcess::spawn(command, pty_cols, pty_rows)?;
    let mut parser = vt100::Parser::new(pty_rows, pty_cols, 1_000);

    loop {
        for bytes in pty.drain_output() {
            parser.process(&bytes);
            for response in terminal_query_responses(&bytes, &parser) {
                pty.write_all(response.as_bytes())?;
            }
        }

        if pty.has_exited()? {
            break;
        }

        let size = terminal_guard.terminal.size()?;
        let (pty_cols, pty_rows) = inner_terminal_size(size.width, size.height);
        let (rows, cols) = parser.screen().size();
        if rows != pty_rows || cols != pty_cols {
            parser.screen_mut().set_size(pty_rows, pty_cols);
            pty.resize(pty_cols, pty_rows)?;
        }

        terminal_guard.terminal.draw(|frame| {
            render_pty(frame.area(), frame, &parser, pty.title(), true);
        })?;

        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) if is_quit(key) => break,
                Event::Key(key) => {
                    if let Some(bytes) = key_to_pty_bytes(key) {
                        pty.write_all(&bytes)?;
                    }
                }
                Event::Resize(cols, rows) => {
                    let (pty_cols, pty_rows) = inner_terminal_size(cols, rows);
                    parser.screen_mut().set_size(pty_rows, pty_cols);
                    pty.resize(pty_cols, pty_rows)?;
                }
                _ => {}
            }
        }
    }

    pty.kill();
    Ok(())
}

pub(super) fn inner_terminal_size(cols: u16, rows: u16) -> (u16, u16) {
    let size = inner_terminal_size_value(cols, rows);
    (size.cols, size.rows)
}

fn inner_terminal_size_value(cols: u16, rows: u16) -> TerminalSize {
    TerminalSize::new(cols.saturating_sub(2), rows.saturating_sub(2))
}

pub(super) fn render_pty(
    area: Rect,
    frame: &mut ratatui::Frame<'_>,
    parser: &vt100::Parser,
    title: &str,
    focused: bool,
) {
    let lines = styled_screen_lines(parser);

    let block = Block::default()
        .title(format!("ami-code {title} spike — Ctrl+Q to quit"))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused {
            Color::LightYellow
        } else {
            Color::Cyan
        }));

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn styled_screen_lines(parser: &vt100::Parser) -> Vec<Line<'static>> {
    let screen = parser.screen();
    let (rows, cols) = screen.size();
    let cursor = screen.cursor_position();
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
            spans.push(Span::styled(
                contents,
                cell_style(cell, cursor == (row, col)),
            ));
        }
        lines.push(Line::from(spans));
    }

    lines
}

fn cell_style(cell: &vt100::Cell, is_cursor: bool) -> Style {
    let mut fg = color_to_ratatui(cell.fgcolor());
    let mut bg = color_to_ratatui(cell.bgcolor());

    if cell.inverse() {
        match (fg, bg) {
            (None, None) => {
                fg = Some(Color::Black);
                bg = Some(Color::White);
            }
            _ => std::mem::swap(&mut fg, &mut bg),
        }
    }

    if is_cursor {
        match (fg, bg) {
            (None, None) => {
                fg = Some(Color::Black);
                bg = Some(Color::LightYellow);
            }
            _ => std::mem::swap(&mut fg, &mut bg),
        }
    }

    let mut style = Style::default();
    if let Some(color) = fg {
        style = style.fg(color);
    }
    if let Some(color) = bg {
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

fn color_to_ratatui(color: vt100::Color) -> Option<Color> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(idx) => Some(indexed_color(idx)),
        vt100::Color::Rgb(red, green, blue) => Some(Color::Rgb(red, green, blue)),
    }
}

fn indexed_color(idx: u8) -> Color {
    match idx {
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

pub(super) fn terminal_query_responses(bytes: &[u8], parser: &vt100::Parser) -> Vec<String> {
    crate::terminal::query_responses(bytes, parser)
}

pub(super) fn is_quit(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL)
}

pub(super) fn key_to_pty_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    crate::terminal::encode_key(key)
}

#[derive(Debug, Clone, Copy)]
pub(super) enum EmbeddedCommand {
    Shell,
    Nvim,
    Pi,
}

impl EmbeddedCommand {
    fn title(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Nvim => "nvim",
            Self::Pi => "pi",
        }
    }

    fn process_spec(self) -> Result<ProcessSpec> {
        let program = match self {
            Self::Shell => std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string()),
            Self::Nvim => "nvim".to_string(),
            Self::Pi => "pi".to_string(),
        };

        Ok(ProcessSpec::new(program)
            .display_name(self.title())
            .env("TERM", "xterm-256color")
            .cwd(std::env::current_dir()?))
    }
}

pub(super) struct PtyProcess {
    command: EmbeddedCommand,
    process: crate::terminal::PtyProcess,
}

impl PtyProcess {
    pub(super) fn spawn(command: EmbeddedCommand, cols: u16, rows: u16) -> Result<Self> {
        let spec = command.process_spec()?;
        let process = crate::terminal::PtyProcess::spawn(&spec, TerminalSize::new(cols, rows))?;
        Ok(Self { command, process })
    }

    pub(super) fn title(&self) -> &'static str {
        self.command.title()
    }

    pub(super) fn drain_output(&self) -> Vec<Vec<u8>> {
        self.process.drain_output()
    }

    pub(super) fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        self.process.write_all(bytes)
    }

    pub(super) fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.process.resize(TerminalSize::new(cols, rows))
    }

    pub(super) fn has_exited(&mut self) -> Result<bool> {
        self.process.has_exited()
    }

    pub(super) fn kill(&mut self) {
        self.process.terminate();
    }
}

pub(super) struct TerminalGuard {
    pub(super) terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
}

impl TerminalGuard {
    pub(super) fn enter() -> Result<Self> {
        enable_raw_mode().context("failed to enable raw mode")?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("failed to create ratatui terminal")?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}
