use crate::terminal::PaneSize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSession {
    pub sidebar_width: u16,
    pub agent_width: u16,
    pub bottom_height: u16,
    pub bottom_visible: bool,
}

impl Default for WorkspaceSession {
    fn default() -> Self {
        Self {
            sidebar_width: 24,
            agent_width: 40,
            bottom_height: 12,
            bottom_visible: true,
        }
    }
}

impl WorkspaceSession {
    pub fn editor_size_hint(&self, terminal: PaneSize) -> PaneSize {
        let cols = terminal
            .cols
            .saturating_sub(self.sidebar_width)
            .saturating_sub(self.agent_width);
        let rows = if self.bottom_visible {
            terminal.rows.saturating_sub(self.bottom_height)
        } else {
            terminal.rows
        };

        PaneSize { cols, rows }
    }
}
