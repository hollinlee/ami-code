use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent,
    MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
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
    SidebarStyle, TerminalPaneStyle, render_compact_workbench, render_sidebar,
    render_terminal_pane, terminal_content_size,
};
use crate::workbench::{
    MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH, MouseTarget, PaneId, WorkbenchLayout,
    WorkbenchLayoutConfig, WorkbenchState, hit_test,
};
use crate::workspace::Workspace;

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(16);
const SCROLLBACK_LINES: usize = 1_000;
const MOUSE_WHEEL_LINES: i32 = 3;
const EDGE_SCROLL_INTERVAL: Duration = Duration::from_millis(50);
const EDGE_SCROLL_LINES: i32 = 1;

pub fn run(mode: LaunchMode) -> Result<()> {
    let workspace = Workspace::discover(std::env::current_dir()?)?;
    let mut terminal = TerminalGuard::enter(mode.is_workbench())?;
    let area = terminal.terminal.size()?.into();
    let mut runtime = AppRuntime::new(mode, workspace, area)?;

    loop {
        runtime.poll_sessions()?;
        if runtime.has_exited_session()? {
            break;
        }
        runtime.tick_mouse_selection();

        let area = terminal.terminal.size()?.into();
        runtime.resize(area)?;
        terminal.terminal.draw(|frame| runtime.render(frame))?;

        if event::poll(EVENT_POLL_INTERVAL)? && !runtime.handle_event(event::read()?)? {
            break;
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct PendingMouseGesture {
    target: MouseTarget,
    down: MouseEvent,
    current: MouseEvent,
    dragging: bool,
    next_edge_scroll: Instant,
}

struct AppRuntime {
    launch_mode: LaunchMode,
    workbench: WorkbenchState,
    layout_config: WorkbenchLayoutConfig,
    layout: Option<WorkbenchLayout>,
    sessions: HashMap<PaneId, TerminalSession>,
    status: Option<String>,
    pending_mouse: Option<PendingMouseGesture>,
}

impl AppRuntime {
    fn new(launch_mode: LaunchMode, workspace: Workspace, area: Rect) -> Result<Self> {
        let mut workbench = WorkbenchState::default();
        let layout_config = WorkbenchLayoutConfig::default();
        let layout = launch_mode.is_workbench().then(|| {
            workbench.update_auto_collapse(area, layout_config);
            WorkbenchLayout::calculate_visible(area, layout_config, workbench.visibility())
        });
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
            status: None,
            pending_mouse: None,
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
            self.workbench
                .update_auto_collapse(area, self.layout_config);
            let layout = WorkbenchLayout::calculate_visible(
                area,
                self.layout_config,
                self.workbench.visibility(),
            );
            if self.layout != Some(layout) {
                self.pending_mouse = None;
                self.clear_selection();
            }
            for (pane, pane_area) in [
                (PaneId::Editor, layout.editor),
                (PaneId::Agent, layout.agent),
                (PaneId::Bottom, layout.bottom),
            ] {
                if pane_area.width >= MIN_TERMINAL_WIDTH
                    && pane_area.height >= MIN_TERMINAL_HEIGHT
                    && let Some(session) = self.sessions.get_mut(&pane)
                {
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
            render_session(
                frame,
                frame.area(),
                session,
                true,
                None,
                self.status.as_deref(),
            );
        }
    }

    fn render_workbench(&self, frame: &mut ratatui::Frame<'_>) {
        let Some(layout) = self.layout else {
            return;
        };
        if layout.compact {
            render_compact_workbench(frame, frame.area());
            return;
        }
        if layout.sidebar.width > 0 && layout.sidebar.height > 0 {
            render_sidebar(
                frame,
                layout.sidebar,
                self.workbench.is_focused(PaneId::Sidebar),
                SidebarStyle::default(),
            );
        }

        for (pane, area) in [
            (PaneId::Editor, layout.editor),
            (PaneId::Agent, layout.agent),
            (PaneId::Bottom, layout.bottom),
        ] {
            if area.width >= MIN_TERMINAL_WIDTH
                && area.height >= MIN_TERMINAL_HEIGHT
                && let Some(session) = self.sessions.get(&pane)
            {
                render_session(
                    frame,
                    area,
                    session,
                    self.workbench.is_focused(pane),
                    self.workbench.selection_range(pane),
                    self.status.as_deref(),
                );
            }
        }
    }

    fn handle_event(&mut self, event: Event) -> Result<bool> {
        match event {
            Event::Key(key) => {
                self.pending_mouse = None;
                if is_quit(key) {
                    return Ok(false);
                }
                if should_copy_selection(key, self.workbench.selection().is_some()) {
                    self.copy_selection();
                    return Ok(true);
                }
                if is_system_paste(key) {
                    self.clear_selection();
                    self.paste_system_clipboard();
                    return Ok(true);
                }

                self.clear_selection();
                let pane = if self.launch_mode.is_workbench() {
                    self.workbench.focused_pane()
                } else {
                    PaneId::Editor
                };
                if let Some(session) = self.sessions.get_mut(&pane) {
                    session.send_key(key)?;
                }
            }
            Event::Paste(contents) => {
                self.pending_mouse = None;
                self.clear_selection();
                self.paste_into_focused(&contents);
            }
            Event::Mouse(mouse) if self.launch_mode.is_workbench() => {
                self.handle_mouse_event(mouse)?;
            }
            _ => {}
        }

        Ok(true)
    }

    fn copy_selection(&mut self) {
        let Some(selection) = self.workbench.selection() else {
            return;
        };
        let Some(session) = self.sessions.get(&selection.pane()) else {
            self.set_status("selected pane is unavailable");
            return;
        };
        let contents = session.selected_text(selection.range());
        match crate::clipboard::write_system(&contents) {
            Ok(()) => self.status = None,
            Err(error) => self.set_status(format!("clipboard error: {error}")),
        }
    }

    fn clear_selection(&mut self) {
        if let Some(selection) = self.workbench.selection()
            && let Some(session) = self.sessions.get_mut(&selection.pane())
        {
            session.reset_scrollback();
        }
        self.workbench.clear_selection();
    }

    fn handle_mouse_event(&mut self, event: MouseEvent) -> Result<()> {
        let Some(layout) = self.layout else {
            return Ok(());
        };
        let target = hit_test(layout, event.column, event.row);

        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(target) = target else {
                    return Ok(());
                };
                self.clear_selection();
                self.workbench.focus_pane(target.pane());
                self.pending_mouse = Some(PendingMouseGesture {
                    target,
                    down: event,
                    current: event,
                    dragging: false,
                    next_edge_scroll: Instant::now() + EDGE_SCROLL_INTERVAL,
                });
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(mut pending) = self.pending_mouse else {
                    return Ok(());
                };
                if !pending.dragging {
                    pending.dragging = self.begin_mouse_selection(pending.target);
                }
                pending.current = event;
                if pending.dragging {
                    self.update_mouse_selection(event);
                }
                self.pending_mouse = Some(pending);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(mut pending) = self.pending_mouse.take() else {
                    return Ok(());
                };
                pending.current = event;
                if pending.dragging {
                    self.update_mouse_selection(event);
                } else {
                    self.finish_mouse_click(pending, event)?;
                }
            }
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => self.route_mouse_wheel(target, event)?,
            MouseEventKind::Moved => self.forward_mouse_motion(target, event)?,
            _ => {}
        }

        Ok(())
    }

    fn begin_mouse_selection(&mut self, target: MouseTarget) -> bool {
        let MouseTarget::Content { pane, row, col } = target else {
            return false;
        };
        let Some(session) = self.sessions.get_mut(&pane) else {
            return false;
        };
        session.begin_selection_view();
        self.workbench
            .begin_selection(session.viewport_point(row, col));
        self.status = None;
        true
    }

    fn update_mouse_selection(&mut self, event: MouseEvent) {
        let Some(layout) = self.layout else {
            return;
        };
        let Some(selection) = self.workbench.selection() else {
            return;
        };
        let pane = selection.pane();
        let Some((row, col)) = pointer_content_position(layout, pane, event) else {
            return;
        };
        let Some(session) = self.sessions.get(&pane) else {
            return;
        };
        self.workbench
            .set_selection_head(session.viewport_point(row, col));
    }

    fn tick_mouse_selection(&mut self) {
        let Some(mut pending) = self.pending_mouse else {
            return;
        };
        let now = Instant::now();
        if !pending.dragging || now < pending.next_edge_scroll {
            return;
        }
        pending.next_edge_scroll = now + EDGE_SCROLL_INTERVAL;
        self.pending_mouse = Some(pending);

        let Some(layout) = self.layout else {
            return;
        };
        let Some(selection) = self.workbench.selection() else {
            return;
        };
        let pane = selection.pane();
        let direction = edge_scroll_direction(layout, pane, pending.current);
        if direction == 0 {
            return;
        }
        let Some(session) = self.sessions.get_mut(&pane) else {
            return;
        };
        if !session.scroll_viewport(direction * EDGE_SCROLL_LINES) {
            return;
        }
        let Some((row, col)) = pointer_content_position(layout, pane, pending.current) else {
            return;
        };
        let head = session.viewport_point(row, col);
        self.workbench.set_selection_head(head);
    }

    fn finish_mouse_click(
        &mut self,
        pending: PendingMouseGesture,
        release: MouseEvent,
    ) -> Result<()> {
        let MouseTarget::Content { pane, row, col } = pending.target else {
            return Ok(());
        };
        let Some(session) = self.sessions.get_mut(&pane) else {
            return Ok(());
        };
        let (release_row, release_col) = self
            .layout
            .and_then(|layout| pointer_content_position(layout, pane, release))
            .unwrap_or((row, col));
        session.send_mouse(mouse_at(pending.down, row, col))?;
        session.send_mouse(mouse_at(release, release_row, release_col))?;
        Ok(())
    }

    fn forward_mouse_motion(
        &mut self,
        target: Option<MouseTarget>,
        event: MouseEvent,
    ) -> Result<()> {
        let Some(MouseTarget::Content { pane, row, col }) = target else {
            return Ok(());
        };
        if let Some(session) = self.sessions.get_mut(&pane) {
            session.send_mouse(mouse_at(event, row, col))?;
        }
        Ok(())
    }

    fn route_mouse_wheel(&mut self, target: Option<MouseTarget>, event: MouseEvent) -> Result<()> {
        let Some(MouseTarget::Content { pane, row, col }) = target else {
            return Ok(());
        };
        let selection_owns_wheel = self
            .workbench
            .selection()
            .is_some_and(|selection| selection.pane() == pane);
        let Some(session) = self.sessions.get_mut(&pane) else {
            return Ok(());
        };
        let lines = match event.kind {
            MouseEventKind::ScrollUp => MOUSE_WHEEL_LINES,
            MouseEventKind::ScrollDown => -MOUSE_WHEEL_LINES,
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => 0,
            _ => return Ok(()),
        };

        if selection_owns_wheel && lines != 0 {
            session.scroll_viewport(lines);
        } else if session.mouse_reporting() {
            session.send_mouse(mouse_at(event, row, col))?;
        } else if lines != 0 {
            session.scroll_viewport(lines);
        }
        Ok(())
    }

    fn paste_system_clipboard(&mut self) {
        match crate::clipboard::read_system() {
            Ok(contents) => self.paste_into_focused(&contents),
            Err(error) => self.set_status(format!("clipboard error: {error}")),
        }
    }

    fn paste_into_focused(&mut self, contents: &str) {
        let pane = if self.launch_mode.is_workbench() {
            self.workbench.focused_pane()
        } else {
            PaneId::Editor
        };
        let Some(session) = self.sessions.get_mut(&pane) else {
            self.set_status("focused pane does not accept paste");
            return;
        };

        match session.send_paste(contents) {
            Ok(()) => self.status = None,
            Err(error) => self.set_status(format!("paste rejected: {error}")),
        }
    }

    fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some(message.into());
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
    selection: Option<crate::terminal::TerminalRange>,
    status: Option<&str>,
) {
    let title = session_title(session.display_name(), status, area.width);
    render_terminal_pane(
        frame,
        area,
        session.screen(),
        &title,
        focused,
        selection,
        TerminalPaneStyle::default(),
    );
}

fn mouse_at(mut event: MouseEvent, row: u16, col: u16) -> MouseEvent {
    event.row = row;
    event.column = col;
    event
}

fn pane_area(layout: WorkbenchLayout, pane: PaneId) -> Option<Rect> {
    match pane {
        PaneId::Editor => Some(layout.editor),
        PaneId::Agent => Some(layout.agent),
        PaneId::Bottom => Some(layout.bottom),
        PaneId::Sidebar => None,
    }
}

fn pointer_content_position(
    layout: WorkbenchLayout,
    pane: PaneId,
    event: MouseEvent,
) -> Option<(u16, u16)> {
    let area = pane_area(layout, pane)?;
    let size = terminal_content_size(area);
    let row = event
        .row
        .saturating_sub(area.y.saturating_add(1))
        .min(size.rows.saturating_sub(1));
    let col = event
        .column
        .saturating_sub(area.x.saturating_add(1))
        .min(size.cols.saturating_sub(1));
    Some((row, col))
}

fn edge_scroll_direction(layout: WorkbenchLayout, pane: PaneId, event: MouseEvent) -> i32 {
    let Some(area) = pane_area(layout, pane) else {
        return 0;
    };
    let content_top = area.y.saturating_add(1);
    let content_bottom = area.bottom().saturating_sub(2);
    if event.row <= content_top {
        1
    } else if event.row >= content_bottom {
        -1
    } else {
        0
    }
}

fn session_title(display_name: &str, status: Option<&str>, pane_width: u16) -> String {
    let base_title = display_name.to_string();
    let available = usize::from(pane_width.saturating_sub(2));
    let remaining = available.saturating_sub(base_title.chars().count() + 3);
    match status.filter(|status| !status.is_empty() && remaining > 0) {
        Some(status) => format!("{base_title} — {}", truncate(status, remaining)),
        None => base_title,
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    if max_chars == 1 {
        return "…".to_string();
    }

    let mut truncated: String = value.chars().take(max_chars - 1).collect();
    truncated.push('…');
    truncated
}

fn is_quit(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn should_copy_selection(key: KeyEvent, has_selection: bool) -> bool {
    key.code == KeyCode::Char('c')
        && (key.modifiers.contains(KeyModifiers::SUPER)
            || (has_selection && key.modifiers.contains(KeyModifiers::CONTROL)))
}

fn is_system_paste(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('v') && key.modifiers.contains(KeyModifiers::SUPER)
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
    _state: TerminalStateGuard,
}

impl TerminalGuard {
    fn enter(capture_mouse: bool) -> Result<Self> {
        let state = TerminalStateGuard::enter(capture_mouse)?;
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
    bracketed_paste: bool,
    keyboard_enhancement: bool,
    mouse_capture: bool,
}

impl TerminalStateGuard {
    fn enter(capture_mouse: bool) -> Result<Self> {
        enable_raw_mode().context("failed to enable raw mode")?;
        let mut state = Self {
            alternate_screen: false,
            bracketed_paste: false,
            keyboard_enhancement: false,
            mouse_capture: false,
        };
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
        state.alternate_screen = true;
        execute!(stdout, EnableBracketedPaste).context("failed to enable bracketed paste")?;
        state.bracketed_paste = true;
        if capture_mouse {
            execute!(
                stdout,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES,)
            )
            .context("failed to enable keyboard enhancement")?;
            state.keyboard_enhancement = true;
            execute!(stdout, EnableMouseCapture).context("failed to enable mouse capture")?;
            state.mouse_capture = true;
        }
        Ok(state)
    }
}

impl Drop for TerminalStateGuard {
    fn drop(&mut self) {
        if self.mouse_capture {
            let _ = execute!(std::io::stdout(), DisableMouseCapture);
        }
        if self.keyboard_enhancement {
            let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
        }
        if self.bracketed_paste {
            let _ = execute!(std::io::stdout(), DisableBracketedPaste);
        }
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
    fn bounds_status_to_available_title_width() {
        assert_eq!(
            session_title("pi", Some("clipboard unavailable"), 20),
            "pi — clipboard un…"
        );
        assert_eq!(session_title("nvim", None, 20), "nvim");
        assert!(!session_title("shell", None, 20).contains("Ctrl+Q"));
    }

    #[test]
    fn recognizes_system_clipboard_keys() {
        assert!(should_copy_selection(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::SUPER),
            false,
        ));
        assert!(should_copy_selection(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            true,
        ));
        assert!(is_system_paste(KeyEvent::new(
            KeyCode::Char('v'),
            KeyModifiers::SUPER
        )));
        assert!(!should_copy_selection(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            false,
        ));
    }

    #[test]
    fn detects_vertical_selection_edges() {
        let layout =
            WorkbenchLayout::calculate(Rect::new(0, 0, 120, 40), WorkbenchLayoutConfig::default());
        let event = |row| MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: layout.editor.x + 2,
            row,
            modifiers: KeyModifiers::NONE,
        };

        assert_eq!(edge_scroll_direction(layout, PaneId::Editor, event(1)), 1);
        assert_eq!(edge_scroll_direction(layout, PaneId::Editor, event(10)), 0);
        assert_eq!(
            edge_scroll_direction(layout, PaneId::Editor, event(layout.editor.bottom() - 2)),
            -1
        );
    }
}
