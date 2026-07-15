use std::cmp::{Ordering, max, min};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalPoint {
    pub row: i64,
    pub col: u16,
}

impl TerminalPoint {
    pub fn new(row: i64, col: u16) -> Self {
        Self { row, col }
    }
}

impl Ord for TerminalPoint {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.row, self.col).cmp(&(other.row, other.col))
    }
}

impl PartialOrd for TerminalPoint {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalRange {
    pub start: TerminalPoint,
    pub end: TerminalPoint,
}

impl TerminalRange {
    pub fn inclusive(first: TerminalPoint, second: TerminalPoint) -> Self {
        Self {
            start: min(first, second),
            end: max(first, second),
        }
    }

    pub fn contains(self, point: TerminalPoint) -> bool {
        self.start <= point && point <= self.end
    }
}

pub fn max_scrollback(screen: &vt100::Screen) -> usize {
    let mut snapshot = screen.clone();
    snapshot.set_scrollback(usize::MAX);
    snapshot.scrollback()
}

pub fn normalize_point(
    screen: &vt100::Screen,
    mut point: TerminalPoint,
    moving_forward: bool,
) -> TerminalPoint {
    let max_history = max_scrollback(screen);
    let mut snapshot = screen.clone();
    let Some(visible_row) = reveal_history_row(&mut snapshot, point.row, max_history) else {
        return point;
    };
    if snapshot
        .cell(visible_row, point.col)
        .is_some_and(vt100::Cell::is_wide_continuation)
    {
        point.col = if moving_forward {
            point.col.saturating_add(1)
        } else {
            point.col.saturating_sub(1)
        };
    }
    point
}

pub fn extract(screen: &vt100::Screen, range: TerminalRange) -> String {
    let max_history = max_scrollback(screen);
    let mut snapshot = screen.clone();
    let Some((start_col, _)) = cell_bounds(&mut snapshot, range.start, max_history) else {
        return String::new();
    };
    let Some((_, end_col)) = cell_bounds(&mut snapshot, range.end, max_history) else {
        return String::new();
    };
    let (_, cols) = screen.size();
    let mut contents = String::new();

    for row in range.start.row..=range.end.row {
        let Some(visible_row) = reveal_history_row(&mut snapshot, row, max_history) else {
            continue;
        };
        let row_start = if row == range.start.row { start_col } else { 0 };
        let row_end = if row == range.end.row { end_col } else { cols };
        if row_end > row_start {
            contents.push_str(&snapshot.contents_between(
                visible_row,
                row_start,
                visible_row,
                row_end,
            ));
        }
        if row < range.end.row && !snapshot.row_wrapped(visible_row) {
            contents.push('\n');
        }
    }

    contents
}

fn cell_bounds(
    snapshot: &mut vt100::Screen,
    point: TerminalPoint,
    max_history: usize,
) -> Option<(u16, u16)> {
    let visible_row = reveal_history_row(snapshot, point.row, max_history)?;
    let (_, cols) = snapshot.size();
    let col = point.col.min(cols.saturating_sub(1));
    let cell = snapshot.cell(visible_row, col)?;
    if cell.is_wide_continuation() {
        Some((col.saturating_sub(1), col.saturating_add(1).min(cols)))
    } else if cell.is_wide() {
        Some((col, col.saturating_add(2).min(cols)))
    } else {
        Some((col, col.saturating_add(1).min(cols)))
    }
}

fn reveal_history_row(
    screen: &mut vt100::Screen,
    history_row: i64,
    max_history: usize,
) -> Option<u16> {
    let (rows, _) = screen.size();
    let max_history = i64::try_from(max_history).unwrap_or(i64::MAX);
    if history_row < -max_history || history_row >= i64::from(rows) {
        return None;
    }

    if history_row < 0 {
        screen.set_scrollback(history_row.unsigned_abs() as usize);
        Some(0)
    } else {
        screen.set_scrollback(0);
        Some(history_row as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_ranges_and_contains_cells() {
        let range = TerminalRange::inclusive(TerminalPoint::new(2, 4), TerminalPoint::new(1, 3));
        assert_eq!(range.start, TerminalPoint::new(1, 3));
        assert_eq!(range.end, TerminalPoint::new(2, 4));
        assert!(range.contains(TerminalPoint::new(2, 0)));
        assert!(!range.contains(TerminalPoint::new(3, 0)));
    }

    #[test]
    fn extracts_wrapped_text_without_soft_newline() {
        let mut parser = vt100::Parser::new(2, 4, 10);
        parser.process(b"abcdef");
        let range = TerminalRange::inclusive(TerminalPoint::new(0, 0), TerminalPoint::new(1, 1));
        assert_eq!(extract(parser.screen(), range), "abcdef");
    }

    #[test]
    fn extracts_real_newlines_and_wide_cells() {
        let mut parser = vt100::Parser::new(3, 6, 10);
        parser.process("ab中文\r\ncd".as_bytes());
        let range = TerminalRange::inclusive(TerminalPoint::new(0, 0), TerminalPoint::new(1, 1));
        assert_eq!(extract(parser.screen(), range), "ab中文\ncd");
    }

    #[test]
    fn includes_complete_wide_cell_from_either_half() {
        let mut parser = vt100::Parser::new(1, 6, 0);
        parser.process("a中b".as_bytes());

        let leading = TerminalRange::inclusive(TerminalPoint::new(0, 1), TerminalPoint::new(0, 1));
        let continuation =
            TerminalRange::inclusive(TerminalPoint::new(0, 2), TerminalPoint::new(0, 2));
        assert_eq!(extract(parser.screen(), leading), "中");
        assert_eq!(extract(parser.screen(), continuation), "中");
    }

    #[test]
    fn extracts_across_scrollback() {
        let mut parser = vt100::Parser::new(2, 5, 10);
        parser.process(b"one\r\ntwo\r\ntri");
        assert!(max_scrollback(parser.screen()) > 0);
        let range = TerminalRange::inclusive(TerminalPoint::new(-1, 0), TerminalPoint::new(1, 2));
        assert_eq!(extract(parser.screen(), range), "one\ntwo\ntri");
    }
}
