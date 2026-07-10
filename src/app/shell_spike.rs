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

use crate::backend::{BackendSpec, NvimBackend, PiBackend, ShellBackend};
use crate::terminal::{ProcessSpec, TerminalSession, TerminalSize};
use crate::ui::{TerminalPaneStyle, render_terminal_pane, terminal_content_size};
use crate::workspace::Workspace;

pub fn run() -> Result<()> {
    run_backend(ShellBackend::system_default())
}

pub fn run_nvim() -> Result<()> {
    run_backend(NvimBackend)
}

pub fn run_pi() -> Result<()> {
    run_backend(PiBackend)
}

fn run_backend(backend: impl BackendSpec) -> Result<()> {
    let workspace = Workspace::discover(std::env::current_dir()?)?;
    let spec = backend.process_spec(&workspace);
    run_session(&spec)
}

fn run_session(spec: &ProcessSpec) -> Result<()> {
    let mut terminal_guard = TerminalGuard::enter()?;
    let size = terminal_guard.terminal.size()?;
    let terminal_size = terminal_content_size(size.into());
    let mut session = TerminalSession::spawn(spec, terminal_size, 1_000)?;

    loop {
        session.poll_output()?;
        if session.has_exited()? {
            break;
        }

        let size = terminal_guard.terminal.size()?;
        session.resize(terminal_content_size(size.into()))?;

        terminal_guard.terminal.draw(|frame| {
            let title = format!("ami-code {} spike — Ctrl+Q to quit", session.display_name());
            render_terminal_pane(
                frame,
                frame.area(),
                session.screen(),
                &title,
                true,
                TerminalPaneStyle::default(),
            );
        })?;

        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) if is_quit(key) => break,
                Event::Key(key) => session.send_key(key)?,
                Event::Resize(cols, rows) => {
                    session.resize(terminal_content_size(Rect::new(0, 0, cols, rows)))?;
                }
                _ => {}
            }
        }
    }

    session.terminate();
    Ok(())
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

    fn process_spec(self, workspace: &Workspace) -> ProcessSpec {
        match self {
            Self::Shell => ShellBackend::system_default().process_spec(workspace),
            Self::Nvim => NvimBackend.process_spec(workspace),
            Self::Pi => PiBackend.process_spec(workspace),
        }
    }
}

pub(super) struct PtyProcess {
    command: EmbeddedCommand,
    process: crate::terminal::PtyProcess,
}

impl PtyProcess {
    pub(super) fn spawn(command: EmbeddedCommand, cols: u16, rows: u16) -> Result<Self> {
        let workspace = Workspace::discover(std::env::current_dir()?)?;
        let spec = command.process_spec(&workspace);
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
