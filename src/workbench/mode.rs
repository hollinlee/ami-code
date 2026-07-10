#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Keyboard input is passed through to the focused backend pane.
    Edit,
    /// The workbench owns keyboard input for focus/layout commands.
    Control,
    /// The workbench owns keyboard input for read-only navigation/selection.
    View,
}
