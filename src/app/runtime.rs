use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
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
use super::supervisor::{RestartDecision, RestartPolicy, SessionIdentity, SessionIds};
use crate::backend::{BackendKind, BackendSpec, NvimBackend, PiBackend, ShellBackend};
use crate::terminal::{PasteError, ProcessSpec, TerminalSession, TerminalSize};
use crate::ui::{
    SidebarStyle, TerminalPaneStyle, render_compact_workbench, render_layout_controls,
    render_sidebar, render_terminal_pane, render_unavailable_terminal_pane, terminal_content_size,
};
use crate::workbench::{
    LayoutDivider, LayoutHandle, LayoutStore, MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH, MouseTarget,
    PaneId, WorkbenchLayout, WorkbenchLayoutConfig, WorkbenchState, hit_test,
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
        runtime.poll_sessions_at(Instant::now())?;
        runtime.tick_mouse_selection();

        let area = terminal.terminal.size()?.into();
        runtime.resize(area)?;
        terminal.terminal.draw(|frame| runtime.render(frame))?;

        if event::poll(EVENT_POLL_INTERVAL)? && !runtime.handle_event(event::read()?)? {
            break;
        }
    }

    runtime.shutdown();
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

#[derive(Debug, Clone, Copy)]
enum PendingLayoutGesture {
    Handle {
        handle: LayoutHandle,
        origin: MouseEvent,
        moved: bool,
    },
    Drag {
        divider: LayoutDivider,
        origin: MouseEvent,
        layout: WorkbenchLayout,
        config: WorkbenchLayoutConfig,
    },
}

enum PasteDelivery {
    Sent,
    Unavailable,
    Rejected(String),
    BackendFailed,
}

enum SlotState {
    Starting,
    Running {
        identity: SessionIdentity,
        started_at: Instant,
        session: Box<TerminalSession>,
    },
    Backoff {
        until: Instant,
        message: String,
    },
    Paused {
        message: String,
    },
}

struct SessionSlot {
    spec: ProcessSpec,
    size: TerminalSize,
    generation: u64,
    policy: RestartPolicy,
    state: SlotState,
}

struct SessionRegistry {
    slots: HashMap<PaneId, SessionSlot>,
    ids: SessionIds,
}

impl SessionRegistry {
    fn single(mode: LaunchMode, workspace: &Workspace, area: Rect, now: Instant) -> Self {
        let spec = match mode {
            LaunchMode::Shell => checked_process_spec(
                ShellBackend::system_default(),
                BackendKind::Shell,
                workspace,
            ),
            LaunchMode::Nvim => checked_process_spec(NvimBackend, BackendKind::Editor, workspace),
            LaunchMode::Pi => checked_process_spec(PiBackend, BackendKind::Agent, workspace),
            LaunchMode::Workbench => unreachable!("workbench uses multiple slots"),
        };
        Self::from_slots([(PaneId::Editor, spec, terminal_content_size(area))], now)
    }

    fn workbench(workspace: &Workspace, layout: WorkbenchLayout, now: Instant) -> Self {
        Self::from_slots(
            [
                (
                    PaneId::Editor,
                    checked_process_spec(NvimBackend, BackendKind::Editor, workspace),
                    terminal_content_size(layout.editor),
                ),
                (
                    PaneId::Agent,
                    checked_process_spec(PiBackend, BackendKind::Agent, workspace),
                    terminal_content_size(layout.agent),
                ),
                (
                    PaneId::Bottom,
                    checked_process_spec(
                        ShellBackend::system_default(),
                        BackendKind::Shell,
                        workspace,
                    ),
                    terminal_content_size(layout.bottom),
                ),
            ],
            now,
        )
    }

    fn from_slots<const N: usize>(
        slots: [(PaneId, ProcessSpec, TerminalSize); N],
        now: Instant,
    ) -> Self {
        let mut registry = Self {
            slots: slots
                .into_iter()
                .map(|(pane, spec, size)| {
                    (
                        pane,
                        SessionSlot {
                            spec,
                            size,
                            generation: 0,
                            policy: RestartPolicy::default(),
                            state: SlotState::Starting,
                        },
                    )
                })
                .collect(),
            ids: SessionIds::new(),
        };
        let panes: Vec<_> = registry.slots.keys().copied().collect();
        for pane in panes {
            registry.spawn(pane, now);
        }
        registry
    }

    fn get(&self, pane: &PaneId) -> Option<&TerminalSession> {
        match &self.slots.get(pane)?.state {
            SlotState::Running { session, .. } => Some(session),
            _ => None,
        }
    }

    fn get_mut(&mut self, pane: &PaneId) -> Option<&mut TerminalSession> {
        match &mut self.slots.get_mut(pane)?.state {
            SlotState::Running { session, .. } => Some(session),
            _ => None,
        }
    }

    fn resize_slot(&mut self, pane: PaneId, size: TerminalSize, now: Instant) -> Result<()> {
        let failure = {
            let Some(slot) = self.slots.get_mut(&pane) else {
                return Ok(());
            };
            slot.size = size;
            match &mut slot.state {
                SlotState::Running {
                    identity, session, ..
                } => session
                    .resize(size)
                    .err()
                    .map(|error| (*identity, format!("backend resize error: {error:#}"))),
                _ => None,
            }
        };
        if let Some((identity, message)) = failure {
            self.handle_failure(pane, identity, now, message)?;
        }
        Ok(())
    }

    fn send_key(&mut self, pane: PaneId, key: KeyEvent, now: Instant) -> Result<bool> {
        let event = {
            let Some(slot) = self.slots.get_mut(&pane) else {
                return Ok(false);
            };
            match &mut slot.state {
                SlotState::Running {
                    identity, session, ..
                } => Some((*identity, session.send_key(key))),
                _ => None,
            }
        };
        let Some((identity, result)) = event else {
            return Ok(false);
        };
        if let Err(error) = result {
            self.handle_failure(
                pane,
                identity,
                now,
                format!("backend input error: {error:#}"),
            )?;
        }
        Ok(true)
    }

    fn send_mouse(&mut self, pane: PaneId, event: MouseEvent, now: Instant) -> Result<bool> {
        let result = {
            let Some(slot) = self.slots.get_mut(&pane) else {
                return Ok(false);
            };
            match &mut slot.state {
                SlotState::Running {
                    identity, session, ..
                } => Some((*identity, session.send_mouse(event))),
                _ => None,
            }
        };
        let Some((identity, result)) = result else {
            return Ok(false);
        };
        if let Err(error) = result {
            self.handle_failure(
                pane,
                identity,
                now,
                format!("backend mouse error: {error:#}"),
            )?;
        }
        Ok(true)
    }

    fn send_paste(&mut self, pane: PaneId, contents: &str, now: Instant) -> Result<PasteDelivery> {
        let result = {
            let Some(slot) = self.slots.get_mut(&pane) else {
                return Ok(PasteDelivery::Unavailable);
            };
            match &mut slot.state {
                SlotState::Running {
                    identity, session, ..
                } => Some((*identity, session.send_paste(contents))),
                _ => None,
            }
        };
        let Some((identity, result)) = result else {
            return Ok(PasteDelivery::Unavailable);
        };
        match result {
            Ok(()) => Ok(PasteDelivery::Sent),
            Err(PasteError::MultilineUnsupported) => Ok(PasteDelivery::Rejected(
                "multi-line paste requires backend bracketed-paste support".to_string(),
            )),
            Err(PasteError::Write(error)) => {
                self.handle_failure(
                    pane,
                    identity,
                    now,
                    format!("backend paste error: {error:#}"),
                )?;
                Ok(PasteDelivery::BackendFailed)
            }
        }
    }

    fn status(&self, pane: PaneId, now: Instant, click_retry: bool) -> Option<String> {
        let slot = self.slots.get(&pane)?;
        match &slot.state {
            SlotState::Running { .. } => None,
            SlotState::Starting => Some("starting…".to_string()),
            SlotState::Backoff { until, message } => Some(format!(
                "{message}; retrying in {}ms ({})",
                until.saturating_duration_since(now).as_millis(),
                retry_hint(click_retry)
            )),
            SlotState::Paused { message } => {
                Some(format!("{message}; paused ({})", retry_hint(click_retry)))
            }
        }
    }

    fn poll(&mut self, now: Instant) -> Result<()> {
        if self.ids.is_shutting_down() {
            return Ok(());
        }
        let panes: Vec<_> = self.slots.keys().copied().collect();
        for pane in panes.iter().copied() {
            let event = {
                let slot = self.slots.get_mut(&pane).expect("known session slot");
                match &mut slot.state {
                    SlotState::Running {
                        identity, session, ..
                    } => {
                        let identity = *identity;
                        match session.poll_output().and_then(|()| session.has_exited()) {
                            Ok(true) => Some((identity, "backend exited".to_string())),
                            Ok(false) => None,
                            Err(error) => Some((identity, format!("backend error: {error:#}"))),
                        }
                    }
                    _ => None,
                }
            };
            if let Some((identity, message)) = event {
                self.handle_failure(pane, identity, now, message)?;
            }
        }
        for pane in panes {
            let due = self.slots.get(&pane).is_some_and(
                |slot| matches!(slot.state, SlotState::Backoff { until, .. } if now >= until),
            );
            if due {
                self.spawn(pane, now);
            }
        }
        Ok(())
    }

    fn handle_failure(
        &mut self,
        pane: PaneId,
        identity: SessionIdentity,
        now: Instant,
        message: String,
    ) -> Result<()> {
        if self.ids.is_shutting_down() {
            return Ok(());
        }
        let slot = self
            .slots
            .get_mut(&pane)
            .context("lifecycle event for unknown session slot")?;
        let (current_identity, started_at) = match &slot.state {
            SlotState::Running {
                identity,
                started_at,
                ..
            } => (*identity, *started_at),
            _ => return Ok(()),
        };
        // Output-reader and wait notifications can race with a replacement. An old
        // process must never remove the new process occupying the same pane.
        if !current_identity.matches(identity) {
            return Ok(());
        }
        if let SlotState::Running { session, .. } = &mut slot.state {
            session.terminate();
        }
        let runtime = now.saturating_duration_since(started_at);
        slot.state = match slot.policy.failed(now, runtime) {
            RestartDecision::Backoff(delay) => SlotState::Backoff {
                until: now + delay,
                message,
            },
            RestartDecision::Paused => SlotState::Paused { message },
        };
        Ok(())
    }

    fn spawn(&mut self, pane: PaneId, now: Instant) {
        if self.ids.is_shutting_down() {
            return;
        }
        let Some(slot) = self.slots.get_mut(&pane) else {
            return;
        };
        slot.generation = slot
            .generation
            .checked_add(1)
            .expect("session generation exhausted");
        let Some(identity) = self.ids.allocate(slot.generation) else {
            return;
        };
        match TerminalSession::spawn(&slot.spec, slot.size, SCROLLBACK_LINES) {
            Ok(session) => {
                slot.state = SlotState::Running {
                    identity,
                    started_at: now,
                    session: Box::new(session),
                };
            }
            Err(error) => {
                let message = format!("failed to start {}: {error:#}", slot.spec.display_name);
                slot.state = match slot.policy.failed(now, Duration::ZERO) {
                    RestartDecision::Backoff(delay) => SlotState::Backoff {
                        until: now + delay,
                        message,
                    },
                    RestartDecision::Paused => SlotState::Paused { message },
                };
            }
        }
    }

    fn retry(&mut self, pane: PaneId, now: Instant) {
        if self.ids.is_shutting_down() || !self.slots.contains_key(&pane) {
            return;
        }
        if let Some(slot) = self.slots.get_mut(&pane) {
            if matches!(slot.state, SlotState::Running { .. }) {
                return;
            }
            slot.policy.reset();
            slot.state = SlotState::Starting;
        }
        self.spawn(pane, now);
    }

    fn shutdown(&mut self) {
        self.ids.shutdown();
        for slot in self.slots.values_mut() {
            if let SlotState::Running { session, .. } = &mut slot.state {
                session.terminate();
            }
            slot.state = SlotState::Paused {
                message: "shut down".to_string(),
            };
        }
    }
}

fn retry_hint(click_retry: bool) -> &'static str {
    if click_retry {
        "click to retry"
    } else {
        "press any key to retry"
    }
}

struct AppRuntime {
    launch_mode: LaunchMode,
    workbench: WorkbenchState,
    layout_config: WorkbenchLayoutConfig,
    layout: Option<WorkbenchLayout>,
    sessions: SessionRegistry,
    status: Option<String>,
    viewport_area: Rect,
    pending_mouse: Option<PendingMouseGesture>,
    pending_layout: Option<PendingLayoutGesture>,
    layout_store: Option<LayoutStore>,
}

impl AppRuntime {
    fn new(launch_mode: LaunchMode, workspace: Workspace, area: Rect) -> Result<Self> {
        let mut workbench = WorkbenchState::default();
        let mut layout_config = WorkbenchLayoutConfig::default();
        let mut status = None;
        let layout_store = if launch_mode.is_workbench() {
            match LayoutStore::from_environment(workspace.root()) {
                Ok(store) => {
                    match store.load() {
                        Ok(Some(intent)) => {
                            layout_config = intent.config;
                            workbench.set_manual_collapse(
                                intent.sidebar_collapsed,
                                intent.bottom_collapsed,
                            );
                        }
                        Ok(None) => {}
                        Err(error) => {
                            status = Some(format!("saved layout ignored: {error:#}"));
                        }
                    }
                    Some(store)
                }
                Err(error) => {
                    status = Some(format!("layout persistence unavailable: {error:#}"));
                    None
                }
            }
        } else {
            None
        };
        // Loaded user intent is installed before this first automatic solve and
        // before SessionRegistry receives initial PTY dimensions.
        let layout = launch_mode.is_workbench().then(|| {
            workbench.update_auto_collapse(area, layout_config);
            WorkbenchLayout::calculate_visible(area, layout_config, workbench.visibility())
        });
        let sessions = if let Some(layout) = layout {
            SessionRegistry::workbench(&workspace, layout, Instant::now())
        } else {
            SessionRegistry::single(launch_mode, &workspace, area, Instant::now())
        };

        Ok(Self {
            launch_mode,
            workbench,
            layout_config,
            layout,
            sessions,
            status,
            viewport_area: area,
            pending_mouse: None,
            pending_layout: None,
            layout_store,
        })
    }

    fn poll_sessions_at(&mut self, now: Instant) -> Result<()> {
        self.sessions.poll(now)
    }

    fn shutdown(&mut self) {
        self.pending_mouse = None;
        self.cancel_layout_gesture();
        self.sessions.shutdown();
    }

    fn resize(&mut self, area: Rect) -> Result<()> {
        self.resize_internal(area, true)
    }

    fn resize_preserving_selection(&mut self, area: Rect) -> Result<()> {
        self.resize_internal(area, false)
    }

    fn resize_internal(&mut self, area: Rect, clear_selection: bool) -> Result<()> {
        let now = Instant::now();
        if self.viewport_area != area {
            self.cancel_layout_gesture();
            self.viewport_area = area;
        }
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
                if clear_selection {
                    self.clear_selection();
                }
            }
            for (pane, pane_area) in [
                (PaneId::Editor, layout.editor),
                (PaneId::Agent, layout.agent),
                (PaneId::Bottom, layout.bottom),
            ] {
                if pane_area.width >= MIN_TERMINAL_WIDTH && pane_area.height >= MIN_TERMINAL_HEIGHT
                {
                    self.sessions
                        .resize_slot(pane, terminal_content_size(pane_area), now)?;
                }
            }
            self.layout = Some(layout);
        } else {
            self.sessions
                .resize_slot(PaneId::Editor, terminal_content_size(area), now)?;
        }
        Ok(())
    }

    fn cancel_layout_gesture(&mut self) {
        if let Some(config) = canceled_layout_config(&mut self.pending_layout) {
            self.layout_config = config;
        }
    }

    fn render(&self, frame: &mut ratatui::Frame<'_>) {
        if self.launch_mode.is_workbench() {
            self.render_workbench(frame);
        } else {
            self.render_slot(frame, PaneId::Editor, frame.area(), true, None);
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
            if area.width >= MIN_TERMINAL_WIDTH && area.height >= MIN_TERMINAL_HEIGHT {
                self.render_slot(
                    frame,
                    pane,
                    area,
                    self.workbench.is_focused(pane),
                    self.workbench.selection_range(pane),
                );
            }
        }
        render_layout_controls(frame, layout);
    }

    fn render_slot(
        &self,
        frame: &mut ratatui::Frame<'_>,
        pane: PaneId,
        area: Rect,
        focused: bool,
        selection: Option<crate::terminal::TerminalRange>,
    ) {
        let title_handle = self.launch_mode.is_workbench()
            && pane == PaneId::Bottom
            && self.layout.is_some_and(|layout| layout.bottom.height > 0);
        if let Some(session) = self.sessions.get(&pane) {
            render_session(
                frame,
                area,
                session,
                focused,
                selection,
                self.status.as_deref(),
                title_handle,
            );
            return;
        }
        let status = self
            .sessions
            .status(pane, Instant::now(), self.launch_mode.is_workbench())
            .unwrap_or_else(|| "unavailable".to_string());
        let display_name = self
            .sessions
            .slots
            .get(&pane)
            .map(|slot| slot.spec.display_name.as_str())
            .unwrap_or("backend");
        let title =
            session_title_with_handle(display_name, Some(&status), area.width, title_handle);
        render_unavailable_terminal_pane(
            frame,
            area,
            &title,
            &status,
            focused,
            TerminalPaneStyle::default(),
        );
    }

    fn handle_event(&mut self, event: Event) -> Result<bool> {
        match event {
            Event::Key(key) => {
                self.pending_mouse = None;
                self.cancel_layout_gesture();
                if is_quit(key) {
                    self.shutdown();
                    return Ok(false);
                }
                if should_copy_selection(key, self.workbench.selection().is_some()) {
                    self.copy_selection();
                    return Ok(true);
                }
                if is_system_paste(key) {
                    self.clear_selection();
                    self.paste_system_clipboard()?;
                    return Ok(true);
                }

                self.clear_selection();
                let pane = if self.launch_mode.is_workbench() {
                    self.workbench.focused_pane()
                } else {
                    PaneId::Editor
                };
                let now = Instant::now();
                if !self.sessions.send_key(pane, key, now)? {
                    self.sessions.retry(pane, now);
                }
            }
            Event::Paste(contents) => {
                self.pending_mouse = None;
                self.cancel_layout_gesture();
                self.clear_selection();
                self.paste_into_focused(&contents)?;
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
                self.cancel_layout_gesture();
                let Some(target) = target else {
                    return Ok(());
                };
                match target {
                    MouseTarget::Handle(handle) => {
                        self.pending_mouse = None;
                        self.pending_layout = Some(PendingLayoutGesture::Handle {
                            handle,
                            origin: event,
                            moved: false,
                        });
                    }
                    MouseTarget::Divider(divider) => {
                        self.pending_mouse = None;
                        self.pending_layout = Some(PendingLayoutGesture::Drag {
                            divider,
                            origin: event,
                            layout,
                            config: self.layout_config,
                        });
                    }
                    _ => {
                        self.pending_layout = None;
                        self.clear_selection();
                        if let Some(pane) = target.pane() {
                            self.workbench.focus_pane(pane);
                        }
                        self.pending_mouse = Some(PendingMouseGesture {
                            target,
                            down: event,
                            current: event,
                            dragging: false,
                            next_edge_scroll: Instant::now() + EDGE_SCROLL_INTERVAL,
                        });
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(gesture) = &mut self.pending_layout {
                    if let PendingLayoutGesture::Handle { origin, moved, .. } = gesture {
                        *moved |= event.column != origin.column || event.row != origin.row;
                    } else {
                        self.update_layout_drag(event)?;
                    }
                    return Ok(());
                }
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
                if let Some(gesture) = self.pending_layout.take() {
                    match gesture {
                        PendingLayoutGesture::Handle { .. } => {
                            if let Some(handle) = activated_layout_handle(gesture, target) {
                                match handle {
                                    LayoutHandle::Sidebar => self.workbench.toggle_sidebar(),
                                    LayoutHandle::Bottom => self.workbench.toggle_bottom(),
                                }
                                self.resize_preserving_selection(self.viewport_area)?;
                                self.save_layout_intent();
                            }
                        }
                        PendingLayoutGesture::Drag { .. } => {
                            // Drag motion updates the preview only. Mouse-up is
                            // the sole divider commit point.
                            self.save_layout_intent();
                        }
                    }
                    return Ok(());
                }
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

    fn update_layout_drag(&mut self, event: MouseEvent) -> Result<()> {
        let Some(PendingLayoutGesture::Drag {
            divider,
            origin,
            layout,
            config,
        }) = self.pending_layout
        else {
            return Ok(());
        };
        self.layout_config = layout_drag_config(divider, origin, event, layout, config);
        self.resize_preserving_selection(self.viewport_area)
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
        if self.sessions.get(&pane).is_none() {
            self.sessions.retry(pane, Instant::now());
            return Ok(());
        }
        let (release_row, release_col) = self
            .layout
            .and_then(|layout| pointer_content_position(layout, pane, release))
            .unwrap_or((row, col));
        let now = Instant::now();
        self.sessions
            .send_mouse(pane, mouse_at(pending.down, row, col), now)?;
        self.sessions
            .send_mouse(pane, mouse_at(release, release_row, release_col), now)?;
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
        self.sessions
            .send_mouse(pane, mouse_at(event, row, col), Instant::now())?;
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
            self.sessions
                .send_mouse(pane, mouse_at(event, row, col), Instant::now())?;
        } else if lines != 0 {
            session.scroll_viewport(lines);
        }
        Ok(())
    }

    fn paste_system_clipboard(&mut self) -> Result<()> {
        match crate::clipboard::read_system() {
            Ok(contents) => self.paste_into_focused(&contents)?,
            Err(error) => self.set_status(format!("clipboard error: {error}")),
        }
        Ok(())
    }

    fn paste_into_focused(&mut self, contents: &str) -> Result<()> {
        let pane = if self.launch_mode.is_workbench() {
            self.workbench.focused_pane()
        } else {
            PaneId::Editor
        };
        match self.sessions.send_paste(pane, contents, Instant::now())? {
            PasteDelivery::Sent => self.status = None,
            PasteDelivery::Unavailable => {
                self.set_status("focused pane does not accept paste");
            }
            PasteDelivery::Rejected(error) => {
                self.set_status(format!("paste rejected: {error}"));
            }
            PasteDelivery::BackendFailed => {
                self.set_status("paste failed; backend will restart");
            }
        }
        Ok(())
    }

    fn save_layout_intent(&mut self) {
        let Some(store) = &self.layout_store else {
            return;
        };
        let intent = self.workbench.layout_intent(self.layout_config);
        match store.save(intent) {
            Ok(())
                if self.status.as_deref().is_some_and(|status| {
                    status.starts_with("failed to save layout:")
                        || status.starts_with("saved layout ignored:")
                }) =>
            {
                self.status = None;
            }
            Ok(()) => {}
            Err(error) => self.set_status(format!("failed to save layout: {error:#}")),
        }
    }

    fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some(message.into());
    }
}

impl Drop for AppRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn checked_process_spec(
    backend: impl BackendSpec,
    expected_kind: BackendKind,
    workspace: &Workspace,
) -> ProcessSpec {
    assert_eq!(
        backend.kind(),
        expected_kind,
        "backend {} has unexpected kind",
        backend.display_name()
    );
    backend.process_spec(workspace)
}

fn render_session(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    session: &TerminalSession,
    focused: bool,
    selection: Option<crate::terminal::TerminalRange>,
    status: Option<&str>,
    title_handle: bool,
) {
    let title = session_title_with_handle(session.display_name(), status, area.width, title_handle);
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

fn activated_layout_handle(
    gesture: PendingLayoutGesture,
    release_target: Option<MouseTarget>,
) -> Option<LayoutHandle> {
    let PendingLayoutGesture::Handle { handle, moved, .. } = gesture else {
        return None;
    };
    (!moved && release_target == Some(MouseTarget::Handle(handle))).then_some(handle)
}

fn canceled_layout_config(
    pending: &mut Option<PendingLayoutGesture>,
) -> Option<WorkbenchLayoutConfig> {
    match pending.take() {
        Some(PendingLayoutGesture::Drag { config, .. }) => Some(config),
        Some(PendingLayoutGesture::Handle { .. }) | None => None,
    }
}

fn layout_drag_config(
    divider: LayoutDivider,
    origin: MouseEvent,
    current: MouseEvent,
    layout: WorkbenchLayout,
    config: WorkbenchLayoutConfig,
) -> WorkbenchLayoutConfig {
    let dx = i32::from(current.column) - i32::from(origin.column);
    let dy = i32::from(current.row) - i32::from(origin.row);
    let mut next = config;
    match divider {
        LayoutDivider::SidebarMain => {
            let total = layout
                .sidebar
                .width
                .saturating_add(layout.editor.width)
                .saturating_add(layout.agent.width);
            let max = total.saturating_sub(MIN_TERMINAL_WIDTH.saturating_mul(2));
            next.sidebar_width = offset_clamped(config.sidebar_width, dx, MIN_TERMINAL_WIDTH, max);
        }
        LayoutDivider::EditorAgent => {
            let main = layout.editor.width.saturating_add(layout.agent.width);
            let width = offset_clamped(
                layout.editor.width,
                dx,
                MIN_TERMINAL_WIDTH,
                main.saturating_sub(MIN_TERMINAL_WIDTH),
            );
            next.editor_width = Some(width);
        }
        LayoutDivider::EditorBottom => {
            let max = layout
                .editor
                .height
                .saturating_add(layout.bottom.height)
                .saturating_sub(MIN_TERMINAL_HEIGHT);
            next.bottom_height =
                offset_clamped(config.bottom_height, -dy, MIN_TERMINAL_HEIGHT, max);
        }
    }
    next
}

fn offset_clamped(start: u16, delta: i32, min: u16, max: u16) -> u16 {
    (i32::from(start) + delta).clamp(i32::from(min), i32::from(max.max(min))) as u16
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

fn session_title_with_handle(
    display_name: &str,
    status: Option<&str>,
    pane_width: u16,
    title_handle: bool,
) -> String {
    let title_width = if title_handle {
        pane_width.saturating_sub(2)
    } else {
        pane_width
    };
    let title = session_title(display_name, status, title_width);
    if title_handle {
        format!("  {title}")
    } else {
        title
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

    fn sleeping_shell_spec() -> ProcessSpec {
        let mut spec = ProcessSpec::new("/bin/sh").display_name("shell");
        spec.args = vec!["-c".to_string(), "sleep 30".to_string()];
        spec
    }

    #[test]
    fn stale_generation_does_not_stop_current_session() {
        let now = Instant::now();
        let mut registry = SessionRegistry::from_slots(
            [(
                PaneId::Editor,
                sleeping_shell_spec(),
                TerminalSize::new(20, 5),
            )],
            now,
        );
        let current = match &registry.slots[&PaneId::Editor].state {
            SlotState::Running { identity, .. } => *identity,
            _ => panic!("session did not start"),
        };
        let stale = SessionIdentity {
            generation: current.generation.saturating_sub(1),
            ..current
        };

        registry
            .handle_failure(PaneId::Editor, stale, now, "stale exit".to_string())
            .unwrap();

        assert!(matches!(
            registry.slots[&PaneId::Editor].state,
            SlotState::Running { identity, .. } if identity == current
        ));
        registry.shutdown();
    }

    #[test]
    fn shutdown_suppresses_retry_and_due_respawn() {
        let now = Instant::now();
        let mut registry = SessionRegistry::from_slots(
            [(
                PaneId::Editor,
                sleeping_shell_spec(),
                TerminalSize::new(20, 5),
            )],
            now,
        );
        registry.shutdown();
        registry.retry(PaneId::Editor, now + Duration::from_secs(1));
        registry.poll(now + Duration::from_secs(60)).unwrap();

        assert!(matches!(
            registry.slots[&PaneId::Editor].state,
            SlotState::Paused { .. }
        ));
    }

    #[test]
    fn unavailable_status_uses_reachable_retry_hint() {
        assert_eq!(retry_hint(true), "click to retry");
        assert_eq!(retry_hint(false), "press any key to retry");
    }

    #[test]
    fn bounds_status_to_available_title_width() {
        assert_eq!(
            session_title("pi", Some("clipboard unavailable"), 20),
            "pi — clipboard un…"
        );
        assert_eq!(session_title("nvim", None, 20), "nvim");
        assert_eq!(
            session_title_with_handle("shell", None, 20, true),
            "  shell"
        );
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

    fn mouse(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn layout_drags_use_snapshot_delta_capture_and_clamp_all_terminals() {
        let config = WorkbenchLayoutConfig::default();
        let layout = WorkbenchLayout::calculate(Rect::new(0, 0, 120, 40), config);

        // A move is calculated from the down snapshot, not a prior move.
        let sidebar = layout_drag_config(
            LayoutDivider::SidebarMain,
            mouse(layout.editor.x, 2),
            mouse(layout.editor.x + 10, 2),
            layout,
            config,
        );
        assert_eq!(sidebar.sidebar_width, config.sidebar_width + 10);

        // The editor/agent divider follows the captured pointer cell exactly.
        let split = layout_drag_config(
            LayoutDivider::EditorAgent,
            mouse(layout.agent.x, 2),
            mouse(layout.agent.x + 7, 2),
            layout,
            config,
        );
        assert_eq!(split.editor_width, Some(layout.editor.width + 7));
        let split_layout = WorkbenchLayout::calculate(Rect::new(0, 0, 120, 40), split);
        assert_eq!(split_layout.editor.width, layout.editor.width + 7);

        // Coordinates far outside the pane remain captured and clamp both sides
        // to a bordered-terminal minimum.
        let clamped_split = layout_drag_config(
            LayoutDivider::EditorAgent,
            mouse(layout.agent.x, 2),
            mouse(u16::MAX, 2),
            layout,
            config,
        );
        let clamped_layout = WorkbenchLayout::calculate(Rect::new(0, 0, 120, 40), clamped_split);
        assert!(clamped_layout.editor.width >= MIN_TERMINAL_WIDTH);
        assert!(clamped_layout.agent.width >= MIN_TERMINAL_WIDTH);

        let bottom = layout_drag_config(
            LayoutDivider::EditorBottom,
            mouse(30, layout.bottom.y),
            mouse(30, 0),
            layout,
            config,
        );
        let bottom_layout = WorkbenchLayout::calculate(Rect::new(0, 0, 120, 40), bottom);
        assert!(bottom_layout.editor.height >= MIN_TERMINAL_HEIGHT);
        assert!(bottom_layout.bottom.height >= MIN_TERMINAL_HEIGHT);
    }

    #[test]
    fn handle_release_requires_an_unmoved_pointer_on_the_same_handle() {
        let origin = mouse(24, 0);
        let handle = PendingLayoutGesture::Handle {
            handle: LayoutHandle::Sidebar,
            origin,
            moved: false,
        };
        assert_eq!(
            activated_layout_handle(handle, Some(MouseTarget::Handle(LayoutHandle::Sidebar))),
            Some(LayoutHandle::Sidebar)
        );

        let moved = PendingLayoutGesture::Handle {
            handle: LayoutHandle::Sidebar,
            origin,
            moved: true,
        };
        assert_eq!(
            activated_layout_handle(moved, Some(MouseTarget::Handle(LayoutHandle::Sidebar))),
            None
        );
        assert_eq!(
            activated_layout_handle(handle, Some(MouseTarget::Sidebar)),
            None
        );
    }

    #[test]
    fn canceled_drag_returns_its_snapshot_for_rollback() {
        let snapshot = WorkbenchLayoutConfig::default();
        let layout = WorkbenchLayout::calculate(Rect::new(0, 0, 120, 40), snapshot);
        let mut pending = Some(PendingLayoutGesture::Drag {
            divider: LayoutDivider::EditorAgent,
            origin: mouse(layout.agent.x, 2),
            layout,
            config: snapshot,
        });

        assert_eq!(canceled_layout_config(&mut pending), Some(snapshot));
        assert!(pending.is_none());

        let mut handle = Some(PendingLayoutGesture::Handle {
            handle: LayoutHandle::Sidebar,
            origin: mouse(1, 0),
            moved: false,
        });
        assert_eq!(canceled_layout_config(&mut handle), None);
        assert!(handle.is_none());
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
