#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneSize {
    pub cols: u16,
    pub rows: u16,
}

impl PaneSize {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self { cols, rows }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyBackendSpec {
    pub command: String,
    pub args: Vec<String>,
}

impl PtyBackendSpec {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
        }
    }
}
