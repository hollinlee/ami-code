use super::PaneId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Down,
    Up,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusGraph;

impl Default for FocusGraph {
    fn default() -> Self {
        Self
    }
}

impl FocusGraph {
    pub fn next(&self, current: PaneId, direction: Direction) -> PaneId {
        match (current, direction) {
            (PaneId::Sidebar, Direction::Right) => PaneId::Editor,
            (PaneId::Editor, Direction::Left) => PaneId::Sidebar,
            (PaneId::Editor, Direction::Right) => PaneId::Agent,
            (PaneId::Agent, Direction::Left) => PaneId::Editor,
            (PaneId::Editor, Direction::Down) => PaneId::Bottom,
            (PaneId::Bottom, Direction::Up) => PaneId::Editor,
            _ => current,
        }
    }
}
