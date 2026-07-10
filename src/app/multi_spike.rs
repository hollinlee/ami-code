use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::shell_spike::{
    EmbeddedCommand, PtyProcess, TerminalGuard, is_quit, key_to_pty_bytes, terminal_query_responses,
};
use crate::ui::{TerminalPaneStyle, render_terminal_pane, terminal_content_size};

pub fn run() -> Result<()> {
    let mut terminal_guard = TerminalGuard::enter()?;
    let mut layout = WorkbenchLayout::calculate(terminal_guard.terminal.size()?.into());

    let mut panes = HashMap::new();
    panes.insert(
        PaneId::Editor,
        PtyPane::spawn(EmbeddedCommand::Nvim, layout.editor)?,
    );
    panes.insert(
        PaneId::Agent,
        PtyPane::spawn(EmbeddedCommand::Pi, layout.agent)?,
    );
    panes.insert(
        PaneId::Bottom,
        PtyPane::spawn(EmbeddedCommand::Shell, layout.bottom)?,
    );

    let mut focused = PaneId::Editor;
    let mut mode = Mode::Edit;

    loop {
        for pane in panes.values_mut() {
            pane.drain_and_process()?;
        }

        if panes
            .values_mut()
            .any(|pane| pane.has_exited().unwrap_or(true))
        {
            break;
        }

        layout = WorkbenchLayout::calculate(terminal_guard.terminal.size()?.into());
        for (id, area) in [
            (PaneId::Editor, layout.editor),
            (PaneId::Agent, layout.agent),
            (PaneId::Bottom, layout.bottom),
        ] {
            if let Some(pane) = panes.get_mut(&id) {
                pane.resize_to_area(area)?;
            }
        }

        terminal_guard.terminal.draw(|frame| {
            render_sidebar(layout.sidebar, frame, focused == PaneId::Sidebar, mode);
            for (id, area) in [
                (PaneId::Editor, layout.editor),
                (PaneId::Agent, layout.agent),
                (PaneId::Bottom, layout.bottom),
            ] {
                if let Some(pane) = panes.get(&id) {
                    let title = format!("ami-code {} spike — Ctrl+Q to quit", pane.pty.title());
                    render_terminal_pane(
                        frame,
                        area,
                        pane.parser.screen(),
                        &title,
                        focused == id,
                        TerminalPaneStyle::default(),
                    );
                }
            }
        })?;

        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) if is_quit(key) => break,
                Event::Key(key) if handle_global_focus_key(key, &mut focused) => {}
                Event::Key(key) if is_control_toggle(key) => mode = mode.toggle_control(),
                Event::Key(key) => match mode {
                    Mode::Control => handle_control_key(key, &mut focused, &mut mode),
                    Mode::Edit => {
                        if let Some(pane) = panes.get_mut(&focused) {
                            if let Some(bytes) = key_to_pty_bytes(key) {
                                pane.pty.write_all(&bytes)?;
                            }
                        }
                    }
                },
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }

    for pane in panes.values_mut() {
        pane.pty.kill();
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PaneId {
    Sidebar,
    Editor,
    Agent,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Edit,
    Control,
}

impl Mode {
    fn toggle_control(self) -> Self {
        match self {
            Self::Edit => Self::Control,
            Self::Control => Self::Edit,
        }
    }
}

struct PtyPane {
    pty: PtyProcess,
    parser: vt100::Parser,
}

impl PtyPane {
    fn spawn(command: EmbeddedCommand, area: Rect) -> Result<Self> {
        let size = terminal_content_size(area);
        Ok(Self {
            pty: PtyProcess::spawn(command, size.cols, size.rows)?,
            parser: vt100::Parser::new(size.rows, size.cols, 1_000),
        })
    }

    fn drain_and_process(&mut self) -> Result<()> {
        for bytes in self.pty.drain_output() {
            self.parser.process(&bytes);
            for response in terminal_query_responses(&bytes, &self.parser) {
                self.pty.write_all(response.as_bytes())?;
            }
        }
        Ok(())
    }

    fn resize_to_area(&mut self, area: Rect) -> Result<()> {
        let size = terminal_content_size(area);
        let (current_rows, current_cols) = self.parser.screen().size();
        if current_rows != size.rows || current_cols != size.cols {
            self.pty.resize(size.cols, size.rows)?;
            self.parser.screen_mut().set_size(size.rows, size.cols);
        }
        Ok(())
    }

    fn has_exited(&mut self) -> Result<bool> {
        self.pty.has_exited()
    }
}

#[derive(Debug, Clone, Copy)]
struct WorkbenchLayout {
    sidebar: Rect,
    editor: Rect,
    agent: Rect,
    bottom: Rect,
}

impl WorkbenchLayout {
    fn calculate(area: Rect) -> Self {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(24),
                Constraint::Min(20),
                Constraint::Length(40),
            ])
            .split(area);

        let middle = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(8), Constraint::Length(12)])
            .split(columns[1]);

        Self {
            sidebar: columns[0],
            editor: middle[0],
            bottom: middle[1],
            agent: columns[2],
        }
    }
}

fn render_sidebar(area: Rect, frame: &mut ratatui::Frame<'_>, focused: bool, mode: Mode) {
    let border = if focused {
        Color::LightYellow
    } else {
        Color::DarkGray
    };
    let text = format!(
        "dummy sidebar\n\nmode: {:?}\n\nCtrl+h/j/k/l focus\nCtrl+Space control\nCtrl+Q quit",
        mode
    );
    let block = Block::default()
        .title("sidebar")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border));
    frame.render_widget(Paragraph::new(text).block(block), area);
}

fn is_control_toggle(key: KeyEvent) -> bool {
    (key.code == KeyCode::Char(' ') && key.modifiers.contains(KeyModifiers::CONTROL))
        || key.code == KeyCode::Null
}

fn handle_global_focus_key(key: KeyEvent, focused: &mut PaneId) -> bool {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }

    match key.code {
        KeyCode::Char('h') => *focused = focus_left(*focused),
        KeyCode::Char('j') => *focused = focus_down(*focused),
        KeyCode::Char('k') => *focused = focus_up(*focused),
        KeyCode::Char('l') => *focused = focus_right(*focused),
        _ => return false,
    }

    true
}

fn handle_control_key(key: KeyEvent, focused: &mut PaneId, mode: &mut Mode) {
    match key.code {
        KeyCode::Esc => *mode = Mode::Edit,
        KeyCode::Char('h') => *focused = focus_left(*focused),
        KeyCode::Char('j') => *focused = focus_down(*focused),
        KeyCode::Char('k') => *focused = focus_up(*focused),
        KeyCode::Char('l') => *focused = focus_right(*focused),
        _ => {}
    }
}

fn focus_left(current: PaneId) -> PaneId {
    match current {
        PaneId::Editor => PaneId::Sidebar,
        PaneId::Agent => PaneId::Editor,
        other => other,
    }
}

fn focus_right(current: PaneId) -> PaneId {
    match current {
        PaneId::Sidebar => PaneId::Editor,
        PaneId::Editor => PaneId::Agent,
        other => other,
    }
}

fn focus_down(current: PaneId) -> PaneId {
    match current {
        PaneId::Editor => PaneId::Bottom,
        other => other,
    }
}

fn focus_up(current: PaneId) -> PaneId {
    match current {
        PaneId::Bottom => PaneId::Editor,
        other => other,
    }
}
