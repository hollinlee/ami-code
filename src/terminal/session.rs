use anyhow::Result;
use crossterm::event::{KeyEvent, MouseEvent};

use super::input::encode_key;
use super::mouse;
use super::paste::{self, PasteError};
use super::process::{ProcessSpec, PtyProcess, TerminalSize};
use super::query;
use super::selection::{self, TerminalPoint, TerminalRange};

pub struct TerminalSession {
    display_name: String,
    process: PtyProcess,
    parser: vt100::Parser,
}

impl TerminalSession {
    pub fn spawn(spec: &ProcessSpec, size: TerminalSize, scrollback_len: usize) -> Result<Self> {
        let display_name = spec.display_name.clone();
        let process = PtyProcess::spawn(spec, size)?;
        let parser = vt100::Parser::new(size.rows, size.cols, scrollback_len);

        Ok(Self {
            display_name,
            process,
            parser,
        })
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    pub fn poll_output(&mut self) -> Result<()> {
        for bytes in self.process.drain_output() {
            self.parser.process(&bytes);
            for response in query::responses(&bytes, &self.parser) {
                self.process.write_all(response.as_bytes())?;
            }
        }
        Ok(())
    }

    pub fn send_key(&mut self, key: KeyEvent) -> Result<()> {
        if let Some(bytes) = encode_key(key) {
            self.process.write_all(&bytes)?;
        }
        Ok(())
    }

    pub fn send_paste(&mut self, contents: &str) -> Result<(), PasteError> {
        let bytes = paste::encode(contents, self.parser.screen().bracketed_paste())?;
        self.process.write_all(&bytes).map_err(PasteError::Write)
    }

    pub fn selection_cursor(&mut self) -> TerminalPoint {
        self.parser.screen_mut().set_scrollback(0);
        let (row, col) = self.parser.screen().cursor_position();
        TerminalPoint::new(i64::from(row), col)
    }

    pub fn viewport_point(&self, row: u16, col: u16) -> TerminalPoint {
        let (rows, cols) = self.parser.screen().size();
        let scrollback = i64::try_from(self.parser.screen().scrollback()).unwrap_or(i64::MAX);
        let point = TerminalPoint::new(
            i64::from(row.min(rows.saturating_sub(1))) - scrollback,
            col.min(cols.saturating_sub(1)),
        );
        selection::normalize_point(self.parser.screen(), point, false)
    }

    pub fn mouse_reporting(&self) -> bool {
        self.parser.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None
    }

    pub fn send_mouse(&mut self, event: MouseEvent) -> Result<()> {
        let screen = self.parser.screen();
        if let Some(bytes) = mouse::encode(
            screen.mouse_protocol_mode(),
            screen.mouse_protocol_encoding(),
            event,
        ) {
            self.process.write_all(&bytes)?;
        }
        Ok(())
    }

    pub fn scroll_viewport(&mut self, lines: i32) {
        let current = i64::try_from(self.parser.screen().scrollback()).unwrap_or(i64::MAX);
        let maximum =
            i64::try_from(selection::max_scrollback(self.parser.screen())).unwrap_or(i64::MAX);
        let target = current.saturating_add(i64::from(lines)).clamp(0, maximum) as usize;
        self.parser.screen_mut().set_scrollback(target);
    }

    pub fn move_selection_point(
        &mut self,
        point: TerminalPoint,
        row_delta: i64,
        col_delta: i32,
    ) -> TerminalPoint {
        let (rows, cols) = self.parser.screen().size();
        let min_row =
            -i64::try_from(selection::max_scrollback(self.parser.screen())).unwrap_or(i64::MAX);
        let row = point
            .row
            .saturating_add(row_delta)
            .clamp(min_row, i64::from(rows.saturating_sub(1)));
        let col = i32::from(point.col)
            .saturating_add(col_delta)
            .clamp(0, i32::from(cols.saturating_sub(1))) as u16;
        let moved = selection::normalize_point(
            self.parser.screen(),
            TerminalPoint::new(row, col),
            col_delta > 0,
        );
        self.reveal(moved);
        moved
    }

    pub fn move_selection_to_line_edge(
        &mut self,
        point: TerminalPoint,
        end: bool,
    ) -> TerminalPoint {
        let col = if end {
            selection::line_end(self.parser.screen(), point)
        } else {
            0
        };
        let moved = TerminalPoint::new(point.row, col);
        self.reveal(moved);
        moved
    }

    pub fn page_rows(&self, full_page: bool) -> i64 {
        let (rows, _) = self.parser.screen().size();
        let rows = i64::from(rows);
        if full_page {
            rows.max(1)
        } else {
            (rows / 2).max(1)
        }
    }

    pub fn selected_text(&self, range: TerminalRange) -> String {
        selection::extract(self.parser.screen(), range)
    }

    pub fn reset_scrollback(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }

    fn reveal(&mut self, point: TerminalPoint) {
        let (rows, _) = self.parser.screen().size();
        let target = scrollback_to_reveal(rows, self.parser.screen().scrollback(), point.row);
        self.parser.screen_mut().set_scrollback(target);
    }

    pub fn resize(&mut self, size: TerminalSize) -> Result<()> {
        let (rows, cols) = self.parser.screen().size();
        if rows != size.rows || cols != size.cols {
            self.process.resize(size)?;
            self.parser.screen_mut().set_size(size.rows, size.cols);
        }
        Ok(())
    }

    pub fn has_exited(&mut self) -> Result<bool> {
        self.process.has_exited()
    }

    pub fn terminate(&mut self) {
        self.process.terminate();
    }
}

fn scrollback_to_reveal(rows: u16, current: usize, point_row: i64) -> usize {
    let current_row = i64::try_from(current).unwrap_or(i64::MAX);
    let live_bottom = i64::from(rows.saturating_sub(1));
    let visible_top = -current_row;
    let visible_bottom = live_bottom - current_row;

    if point_row < visible_top {
        point_row.unsigned_abs() as usize
    } else if point_row > visible_bottom {
        usize::try_from(live_bottom.saturating_sub(point_row).max(0)).unwrap_or(0)
    } else {
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveals_points_above_and_below_viewport() {
        assert_eq!(scrollback_to_reveal(20, 10, -12), 12);
        assert_eq!(scrollback_to_reveal(20, 10, 15), 4);
        assert_eq!(scrollback_to_reveal(20, 10, 5), 10);
    }

    #[test]
    fn clamps_invalid_points_to_live_viewport() {
        assert_eq!(scrollback_to_reveal(20, 10, 25), 0);
    }
}
