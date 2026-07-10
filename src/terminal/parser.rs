use super::{PaneSize, TerminalScreen};

#[derive(Debug)]
pub struct TerminalParser {
    size: PaneSize,
}

impl TerminalParser {
    pub fn new(size: PaneSize) -> Self {
        Self { size }
    }

    pub fn size(&self) -> PaneSize {
        self.size
    }

    pub fn resize(&mut self, size: PaneSize) {
        self.size = size;
    }

    pub fn parse(&mut self, _bytes: &[u8]) -> TerminalScreen {
        TerminalScreen::empty(self.size)
    }
}
