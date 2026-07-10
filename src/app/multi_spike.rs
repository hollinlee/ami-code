use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use super::shell_spike::{
    EmbeddedCommand, PtyProcess, TerminalGuard, is_quit, key_to_pty_bytes, terminal_query_responses,
};
use crate::ui::{
    SidebarStyle, TerminalPaneStyle, render_sidebar, render_terminal_pane, terminal_content_size,
};
use crate::workbench::{
    Direction, Mode, PaneId, WorkbenchLayout, WorkbenchLayoutConfig, WorkbenchState,
};

pub fn run() -> Result<()> {
    let mut terminal_guard = TerminalGuard::enter()?;
    let layout_config = WorkbenchLayoutConfig::default();
    let mut layout =
        WorkbenchLayout::calculate(terminal_guard.terminal.size()?.into(), layout_config);

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

    let mut workbench = WorkbenchState::default();

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

        layout = WorkbenchLayout::calculate(terminal_guard.terminal.size()?.into(), layout_config);
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
            render_sidebar(
                frame,
                layout.sidebar,
                workbench.is_focused(PaneId::Sidebar),
                workbench.mode(),
                SidebarStyle::default(),
            );
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
                        workbench.is_focused(id),
                        TerminalPaneStyle::default(),
                    );
                }
            }
        })?;

        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) if is_quit(key) => break,
                Event::Key(key) => handle_key(key, &mut workbench, &mut panes)?,
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

fn handle_key(
    key: KeyEvent,
    workbench: &mut WorkbenchState,
    panes: &mut HashMap<PaneId, PtyPane>,
) -> Result<()> {
    if let Some(direction) = global_focus_direction(key) {
        workbench.focus(direction);
        return Ok(());
    }

    if is_control_toggle(key) {
        workbench.toggle_control_mode();
        return Ok(());
    }

    match workbench.mode() {
        Mode::Edit => {
            if let Some(pane) = panes.get_mut(&workbench.focused_pane())
                && let Some(bytes) = key_to_pty_bytes(key)
            {
                pane.pty.write_all(&bytes)?;
            }
        }
        Mode::Control | Mode::View => handle_workbench_key(key, workbench),
    }

    Ok(())
}

fn global_focus_direction(key: KeyEvent) -> Option<Direction> {
    key.modifiers
        .contains(KeyModifiers::CONTROL)
        .then(|| direction_for_key(key.code))
        .flatten()
}

fn direction_for_key(code: KeyCode) -> Option<Direction> {
    match code {
        KeyCode::Char('h') => Some(Direction::Left),
        KeyCode::Char('j') => Some(Direction::Down),
        KeyCode::Char('k') => Some(Direction::Up),
        KeyCode::Char('l') => Some(Direction::Right),
        _ => None,
    }
}

fn is_control_toggle(key: KeyEvent) -> bool {
    (key.code == KeyCode::Char(' ') && key.modifiers.contains(KeyModifiers::CONTROL))
        || key.code == KeyCode::Null
}

fn handle_workbench_key(key: KeyEvent, workbench: &mut WorkbenchState) {
    match key.code {
        KeyCode::Esc => workbench.set_mode(Mode::Edit),
        code => {
            if let Some(direction) = direction_for_key(code) {
                workbench.focus(direction);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_vim_direction_keys() {
        assert_eq!(direction_for_key(KeyCode::Char('h')), Some(Direction::Left));
        assert_eq!(direction_for_key(KeyCode::Char('j')), Some(Direction::Down));
        assert_eq!(direction_for_key(KeyCode::Char('k')), Some(Direction::Up));
        assert_eq!(
            direction_for_key(KeyCode::Char('l')),
            Some(Direction::Right)
        );
        assert_eq!(direction_for_key(KeyCode::Char('x')), None);
    }
}
