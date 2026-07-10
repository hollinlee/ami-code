use anyhow::Result;
use crossterm::event::KeyEvent;

use super::input::encode_key;
use super::process::{ProcessSpec, PtyProcess, TerminalSize};
use super::query;

pub struct TerminalSession {
    display_name: String,
    process: PtyProcess,
    parser: vt100::Parser,
}

impl TerminalSession {
    pub fn spawn(spec: ProcessSpec, size: TerminalSize, scrollback_len: usize) -> Result<Self> {
        let display_name = spec.display_name.clone();
        let process = PtyProcess::spawn(&spec, size)?;
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

    pub fn parser(&self) -> &vt100::Parser {
        &self.parser
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

    pub fn send_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.process.write_all(bytes)
    }

    pub fn resize(&mut self, size: TerminalSize) -> Result<()> {
        let (rows, cols) = self.parser.screen().size();
        if rows != size.rows || cols != size.cols {
            self.parser.screen_mut().set_size(size.rows, size.cols);
            self.process.resize(size)?;
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
