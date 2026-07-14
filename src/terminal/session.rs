use anyhow::Result;
use crossterm::event::KeyEvent;

use super::input::encode_key;
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
        let current = i64::try_from(self.parser.screen().scrollback()).unwrap_or(i64::MAX);
        let top = -current;
        let bottom = i64::from(rows.saturating_sub(1)) - current;
        let target = if point.row < top {
            point.row.unsigned_abs() as usize
        } else if point.row > bottom {
            usize::try_from(i64::from(rows.saturating_sub(1)) - point.row).unwrap_or(0)
        } else {
            self.parser.screen().scrollback()
        };
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
