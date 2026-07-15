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
    selection_screen: Option<vt100::Screen>,
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
            selection_screen: None,
        })
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.selection_screen
            .as_ref()
            .unwrap_or_else(|| self.parser.screen())
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

    pub fn begin_selection_view(&mut self) {
        self.selection_screen = Some(self.parser.screen().clone());
    }

    pub fn viewport_point(&self, row: u16, col: u16) -> TerminalPoint {
        let screen = self.screen();
        let (rows, cols) = screen.size();
        let scrollback = i64::try_from(screen.scrollback()).unwrap_or(i64::MAX);
        let point = TerminalPoint::new(
            i64::from(row.min(rows.saturating_sub(1))) - scrollback,
            col.min(cols.saturating_sub(1)),
        );
        selection::normalize_point(screen, point, false)
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

    pub fn scroll_viewport(&mut self, lines: i32) -> bool {
        let screen = self.screen();
        let current = i64::try_from(screen.scrollback()).unwrap_or(i64::MAX);
        let maximum = i64::try_from(selection::max_scrollback(screen)).unwrap_or(i64::MAX);
        let target = viewport_scroll_target(current, maximum, lines);
        if target == screen.scrollback() {
            return false;
        }
        if let Some(screen) = &mut self.selection_screen {
            screen.set_scrollback(target);
        } else {
            self.parser.screen_mut().set_scrollback(target);
        }
        true
    }

    pub fn selected_text(&self, range: TerminalRange) -> String {
        selection::extract(self.screen(), range)
    }

    pub fn reset_scrollback(&mut self) {
        self.selection_screen = None;
        self.parser.screen_mut().set_scrollback(0);
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

fn viewport_scroll_target(current: i64, maximum: i64, lines: i32) -> usize {
    current.saturating_add(i64::from(lines)).clamp(0, maximum) as usize
}

#[cfg(test)]
mod tests {
    use super::viewport_scroll_target;

    #[test]
    fn clamps_viewport_scroll_to_history_bounds() {
        assert_eq!(viewport_scroll_target(5, 10, 3), 8);
        assert_eq!(viewport_scroll_target(5, 10, -8), 0);
        assert_eq!(viewport_scroll_target(8, 10, 8), 10);
    }
}
