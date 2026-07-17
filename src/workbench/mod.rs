mod hit_test;
mod layout;
mod pane;
mod persistence;
mod selection;
mod state;
mod tabs;

pub use hit_test::{LayoutDivider, LayoutHandle, MouseTarget, hit_test, layout_handle_position};
pub use layout::{
    MIN_SHELL_PANE_HEIGHT, MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH, WorkbenchLayout,
    WorkbenchLayoutConfig, WorkbenchVisibility,
};
pub use pane::{PaneId, PaneKind, PaneState};
pub use persistence::{LayoutIntent, LayoutStore};
pub use selection::PaneSelection;
pub use state::WorkbenchState;
pub use tabs::{ShellTabId, ShellTabTarget, ShellTabs, shell_tab_geometry, shell_tab_hit_test};
