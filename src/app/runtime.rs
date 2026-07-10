use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;

use super::LaunchMode;
use crate::backend::{BackendKind, BackendSpec, NvimBackend, PiBackend, ShellBackend};
use crate::terminal::TerminalSession;
use crate::ui::{
    SidebarStyle, TerminalPaneStyle, render_sidebar, render_terminal_pane, terminal_content_size,
};
use crate::workbench::{
    Direction, Mode, PaneId, WorkbenchLayout, WorkbenchLayoutConfig, WorkbenchState,
};
use crate::workspace::Workspace;

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(16);
const SCROLLBACK_LINES: usize = 1_000;

pub fn run(mode: LaunchMode) -> Result<()> {
    let workspace = Workspace::discover(std::env::current_dir()?)?;
    let mut terminal = TerminalGuard::enter()?;
    let area = terminal.terminal.size()?.into();
    let mut runtime = AppRuntime::new(mode, workspace, area)?;

    loop {
        runtime.poll_sessions()?;
        if runtime.has_exited_session()? {
            break;
        }

        let area = terminal.terminal.size()?.into();
        runtime.resize(area)?;
        terminal.terminal.draw(|frame| runtime.render(frame))?;

        if event::poll(EVENT_POLL_INTERVAL)? && !runtime.handle_event(event::read()?)? {
            break;
        }
    }

    Ok(())
}

struct AppRuntime {
    launch_mode: LaunchMode,
    workbench: WorkbenchState,
    layout_config: WorkbenchLayoutConfig,
    layout: Option<WorkbenchLayout>,
    sessions: HashMap<PaneId, TerminalSession>,
}

impl AppRuntime {
    fn new(launch_mode: LaunchMode, workspace: Workspace, area: Rect) -> Result<Self> {
        let workbench = WorkbenchState::default();
        let layout_config = WorkbenchLayoutConfig::default();
        let layout = launch_mode
            .is_workbench()
            .then(|| WorkbenchLayout::calculate(area, layout_config));
        let sessions = if let Some(layout) = layout {
            Self::spawn_workbench_sessions(&workspace, layout)?
        } else {
            Self::spawn_single_session(launch_mode, &workspace, area)?
        };

        Ok(Self {
            launch_mode,
            workbench,
            layout_config,
            layout,
            sessions,
        })
    }

    fn spawn_single_session(
        launch_mode: LaunchMode,
        workspace: &Workspace,
        area: Rect,
    ) -> Result<HashMap<PaneId, TerminalSession>> {
        let session = match launch_mode {
            LaunchMode::Shell => spawn_backend_session(
                ShellBackend::system_default(),
                BackendKind::Shell,
                workspace,
                area,
            )?,
            LaunchMode::Nvim => {
                spawn_backend_session(NvimBackend, BackendKind::Editor, workspace, area)?
            }
            LaunchMode::Pi => {
                spawn_backend_session(PiBackend, BackendKind::Agent, workspace, area)?
            }
            LaunchMode::Workbench => unreachable!("workbench uses multiple sessions"),
        };
        Ok(HashMap::from([(PaneId::Editor, session)]))
    }

    fn spawn_workbench_sessions(
        workspace: &Workspace,
        layout: WorkbenchLayout,
    ) -> Result<HashMap<PaneId, TerminalSession>> {
        Ok(HashMap::from([
            (
                PaneId::Editor,
                spawn_backend_session(NvimBackend, BackendKind::Editor, workspace, layout.editor)?,
            ),
            (
                PaneId::Agent,
                spawn_backend_session(PiBackend, BackendKind::Agent, workspace, layout.agent)?,
            ),
            (
                PaneId::Bottom,
                spawn_backend_session(
                    ShellBackend::system_default(),
                    BackendKind::Shell,
                    workspace,
                    layout.bottom,
                )?,
            ),
        ]))
    }

    fn poll_sessions(&mut self) -> Result<()> {
        for session in self.sessions.values_mut() {
            session.poll_output()?;
        }
        Ok(())
    }

    fn has_exited_session(&mut self) -> Result<bool> {
        for session in self.sessions.values_mut() {
            if session.has_exited()? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn resize(&mut self, area: Rect) -> Result<()> {
        if self.launch_mode.is_workbench() {
            let layout = WorkbenchLayout::calculate(area, self.layout_config);
            for (pane, pane_area) in [
                (PaneId::Editor, layout.editor),
                (PaneId::Agent, layout.agent),
                (PaneId::Bottom, layout.bottom),
            ] {
                if let Some(session) = self.sessions.get_mut(&pane) {
                    session.resize(terminal_content_size(pane_area))?;
                }
            }
            self.layout = Some(layout);
        } else if let Some(session) = self.sessions.get_mut(&PaneId::Editor) {
            session.resize(terminal_content_size(area))?;
        }
        Ok(())
    }

    fn render(&self, frame: &mut ratatui::Frame<'_>) {
        if self.launch_mode.is_workbench() {
            self.render_workbench(frame);
        } else if let Some(session) = self.sessions.get(&PaneId::Editor) {
            render_session(frame, frame.area(), session, true);
        }
    }

    fn render_workbench(&self, frame: &mut ratatui::Frame<'_>) {
        let Some(layout) = self.layout else {
            return;
        };
        render_sidebar(
            frame,
            layout.sidebar,
            self.workbench.is_focused(PaneId::Sidebar),
            self.workbench.mode(),
            SidebarStyle::default(),
        );

        for (pane, area) in [
            (PaneId::Editor, layout.editor),
            (PaneId::Agent, layout.agent),
            (PaneId::Bottom, layout.bottom),
        ] {
            if let Some(session) = self.sessions.get(&pane) {
                render_session(frame, area, session, self.workbench.is_focused(pane));
            }
        }
    }

    fn handle_event(&mut self, event: Event) -> Result<bool> {
        let Event::Key(key) = event else {
            return Ok(true);
        };

        if is_quit(key) {
            return Ok(false);
        }

        if self.launch_mode.is_workbench() {
            self.handle_workbench_key(key)?;
        } else if let Some(session) = self.sessions.get_mut(&PaneId::Editor) {
            session.send_key(key)?;
        }

        Ok(true)
    }

    fn handle_workbench_key(&mut self, key: KeyEvent) -> Result<()> {
        if let Some(direction) = global_focus_direction(key) {
            self.workbench.focus(direction);
            return Ok(());
        }

        if is_control_toggle(key) {
            self.workbench.toggle_control_mode();
            return Ok(());
        }

        match self.workbench.mode() {
            Mode::Edit => {
                if let Some(session) = self.sessions.get_mut(&self.workbench.focused_pane()) {
                    session.send_key(key)?;
                }
            }
            Mode::Control | Mode::View => match key.code {
                KeyCode::Esc => self.workbench.set_mode(Mode::Edit),
                KeyCode::Char('v') => self.workbench.set_mode(Mode::View),
                code => {
                    if let Some(direction) = direction_for_key(code) {
                        self.workbench.focus(direction);
                    }
                }
            },
        }

        Ok(())
    }
}

impl Drop for AppRuntime {
    fn drop(&mut self) {
        for session in self.sessions.values_mut() {
            session.terminate();
        }
    }
}

fn spawn_backend_session(
    backend: impl BackendSpec,
    expected_kind: BackendKind,
    workspace: &Workspace,
    area: Rect,
) -> Result<TerminalSession> {
    ensure!(
        backend.kind() == expected_kind,
        "backend {} has unexpected kind",
        backend.display_name()
    );
    let spec = backend.process_spec(workspace);
    TerminalSession::spawn(&spec, terminal_content_size(area), SCROLLBACK_LINES)
}

fn render_session(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    session: &TerminalSession,
    focused: bool,
) {
    let title = format!("ami-code {} — Ctrl+Q to quit", session.display_name());
    render_terminal_pane(
        frame,
        area,
        session.screen(),
        &title,
        focused,
        TerminalPaneStyle::default(),
    );
}

fn is_quit(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL)
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

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
    _state: TerminalStateGuard,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        let state = TerminalStateGuard::enter()?;
        let backend = CrosstermBackend::new(std::io::stdout());
        let terminal = Terminal::new(backend).context("failed to create ratatui terminal")?;
        Ok(Self {
            terminal,
            _state: state,
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.terminal.show_cursor();
    }
}

struct TerminalStateGuard {
    alternate_screen: bool,
}

impl TerminalStateGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("failed to enable raw mode")?;
        let mut state = Self {
            alternate_screen: false,
        };
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
        state.alternate_screen = true;
        Ok(state)
    }
}

impl Drop for TerminalStateGuard {
    fn drop(&mut self) {
        if self.alternate_screen {
            let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
        }
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_vim_focus_keys() {
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
