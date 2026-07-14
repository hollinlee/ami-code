use crate::terminal::{TerminalPoint, TerminalRange};

use super::PaneId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneSelection {
    pane: PaneId,
    anchor: TerminalPoint,
    head: TerminalPoint,
}

impl PaneSelection {
    pub fn new(pane: PaneId, anchor: TerminalPoint) -> Self {
        Self {
            pane,
            anchor,
            head: anchor,
        }
    }

    pub fn pane(self) -> PaneId {
        self.pane
    }

    pub fn head(self) -> TerminalPoint {
        self.head
    }

    pub fn set_head(&mut self, head: TerminalPoint) {
        self.head = head;
    }

    pub fn range(self) -> TerminalRange {
        TerminalRange::inclusive(self.anchor, self.head)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_anchor_while_head_moves() {
        let anchor = TerminalPoint::new(1, 2);
        let mut selection = PaneSelection::new(PaneId::Editor, anchor);
        selection.set_head(TerminalPoint::new(0, 4));

        assert_eq!(selection.pane(), PaneId::Editor);
        assert_eq!(selection.head(), TerminalPoint::new(0, 4));
        assert_eq!(selection.range().start, TerminalPoint::new(0, 4));
        assert_eq!(selection.range().end, anchor);
    }
}
