use super::PaneSize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalScreen {
    pub size: PaneSize,
    pub lines: Vec<String>,
}

impl TerminalScreen {
    pub fn empty(size: PaneSize) -> Self {
        Self {
            size,
            lines: Vec::new(),
        }
    }
}
