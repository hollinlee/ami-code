use super::PaneId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Down,
    Up,
    Right,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FocusGraph;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_workbench_focus_graph() {
        let graph = FocusGraph;

        assert_eq!(
            graph.next(PaneId::Sidebar, Direction::Right),
            PaneId::Editor
        );
        assert_eq!(graph.next(PaneId::Editor, Direction::Left), PaneId::Sidebar);
        assert_eq!(graph.next(PaneId::Editor, Direction::Right), PaneId::Agent);
        assert_eq!(graph.next(PaneId::Agent, Direction::Left), PaneId::Editor);
        assert_eq!(graph.next(PaneId::Editor, Direction::Down), PaneId::Bottom);
        assert_eq!(graph.next(PaneId::Bottom, Direction::Up), PaneId::Editor);
    }

    #[test]
    fn keeps_focus_when_no_edge_exists() {
        let graph = FocusGraph;
        assert_eq!(graph.next(PaneId::Agent, Direction::Right), PaneId::Agent);
        assert_eq!(
            graph.next(PaneId::Sidebar, Direction::Left),
            PaneId::Sidebar
        );
    }
}
