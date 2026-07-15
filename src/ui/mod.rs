mod sidebar;
mod terminal;

pub use sidebar::{SidebarStyle, render_sidebar};
pub use terminal::{
    TerminalPaneStyle, render_compact_workbench, render_terminal_pane, terminal_content_size,
};
