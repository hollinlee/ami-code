use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

pub fn run() -> Result<()> {
    run_command(EmbeddedCommand::Shell)
}

pub fn run_nvim() -> Result<()> {
    run_command(EmbeddedCommand::Nvim)
}

pub fn run_pi() -> Result<()> {
    run_command(EmbeddedCommand::Pi)
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
    (cols.saturating_sub(2).max(1), rows.saturating_sub(2).max(1))
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
    let mut responses = Vec::new();

    if contains_sequence(bytes, b"\x1b[6n") {
        let (row, col) = parser.screen().cursor_position();
        responses.push(format!(
            "\x1b[{};{}R",
            row.saturating_add(1),
            col.saturating_add(1)
        ));
    }

    if contains_sequence(bytes, b"\x1b[5n") {
        responses.push("\x1b[0n".to_string());
    }

    if contains_sequence(bytes, b"\x1b[c") || contains_sequence(bytes, b"\x1b[0c") {
        responses.push("\x1b[?1;2c".to_string());
    }

    if contains_sequence(bytes, b"\x1b[>c") || contains_sequence(bytes, b"\x1b[>0c") {
        responses.push("\x1b[>0;0;0c".to_string());
    }

    responses
}

fn contains_sequence(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|window| window == needle)
}

pub(super) fn is_quit(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL)
}

pub(super) fn key_to_pty_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                ctrl_char(c).map(|byte| vec![byte])
            } else {
                Some(c.to_string().into_bytes())
            }
        }
        KeyCode::Enter => Some(b"\r".to_vec()),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Tab => Some(b"\t".to_vec()),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        _ => None,
    }
}

fn ctrl_char(c: char) -> Option<u8> {
    let lower = c.to_ascii_lowercase();
    if lower.is_ascii_lowercase() {
        Some((lower as u8) - b'a' + 1)
    } else {
        None
    }
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

    fn command(self) -> String {
        match self {
            Self::Shell => std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string()),
            Self::Nvim => "nvim".to_string(),
            Self::Pi => "pi".to_string(),
        }
    }
}

pub(super) struct PtyProcess {
    command: EmbeddedCommand,
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send>,
    rx: Receiver<Vec<u8>>,
}

impl PtyProcess {
    pub(super) fn spawn(command: EmbeddedCommand, cols: u16, rows: u16) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to open PTY")?;

        let mut builder = CommandBuilder::new(command.command());
        builder.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(builder)
            .with_context(|| format!("failed to spawn {}", command.title()))?;
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone PTY reader")?;
        let writer = pair
            .master
            .take_writer()
            .context("failed to take PTY writer")?;

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buffer[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            command,
            master: pair.master,
            writer,
            child,
            rx,
        })
    }

    pub(super) fn title(&self) -> &'static str {
        self.command.title()
    }

    pub(super) fn drain_output(&self) -> Vec<Vec<u8>> {
        let mut chunks = Vec::new();
        while let Ok(bytes) = self.rx.try_recv() {
            chunks.push(bytes);
        }
        chunks
    }

    pub(super) fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer
            .write_all(bytes)
            .context("failed to write to PTY")
    }

    pub(super) fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to resize PTY")
    }

    pub(super) fn has_exited(&mut self) -> Result<bool> {
        self.child
            .try_wait()
            .map(|status| status.is_some())
            .context("failed to poll PTY child")
    }

    pub(super) fn kill(&mut self) {
        let _ = self.child.kill();
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
