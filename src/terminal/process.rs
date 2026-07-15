use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
}

impl TerminalSize {
    pub fn new(cols: u16, rows: u16) -> Self {
        // vt100 0.16 can underflow while wrapping/scrolling a 1-row or
        // 1-column screen. Keep hidden/compact sessions at a parser-safe size.
        Self {
            cols: cols.max(2),
            rows: rows.max(2),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub display_name: String,
}

impl ProcessSpec {
    pub fn new(program: impl Into<String>) -> Self {
        let program = program.into();
        Self {
            display_name: program.clone(),
            program,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
        }
    }

    pub fn display_name(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = display_name.into();
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
}

pub struct PtyProcess {
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Option<Box<dyn portable_pty::Child + Send>>,
    rx: Receiver<Vec<u8>>,
}

impl PtyProcess {
    pub fn spawn(spec: &ProcessSpec, size: TerminalSize) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(to_pty_size(size))
            .context("failed to open PTY")?;

        let mut command = CommandBuilder::new(&spec.program);
        command.args(&spec.args);
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        if let Some(cwd) = &spec.cwd {
            command.cwd(cwd);
        }

        let child = pair
            .slave
            .spawn_command(command)
            .with_context(|| format!("failed to spawn {}", spec.display_name))?;
        drop(pair.slave);

        let mut reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(error) => {
                terminate_and_reap(child);
                return Err(error).context("failed to clone PTY reader");
            }
        };
        let writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(error) => {
                terminate_and_reap(child);
                return Err(error).context("failed to take PTY writer");
            }
        };

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        if tx.send(buffer[..read].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            master: pair.master,
            writer,
            child: Some(child),
            rx,
        })
    }

    pub fn drain_output(&self) -> Vec<Vec<u8>> {
        let mut chunks = Vec::new();
        while let Ok(bytes) = self.rx.try_recv() {
            chunks.push(bytes);
        }
        chunks
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer
            .write_all(bytes)
            .context("failed to write to PTY")
    }

    pub fn resize(&self, size: TerminalSize) -> Result<()> {
        self.master
            .resize(to_pty_size(size))
            .context("failed to resize PTY")
    }

    pub fn has_exited(&mut self) -> Result<bool> {
        let Some(child) = self.child.as_mut() else {
            return Ok(true);
        };
        child
            .try_wait()
            .map(|status| status.is_some())
            .context("failed to poll PTY child")
    }

    pub fn terminate(&mut self) {
        if let Some(child) = self.child.take() {
            terminate_and_reap(child);
        }
    }
}

fn terminate_and_reap(mut child: Box<dyn portable_pty::Child + Send>) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    let _ = child.kill();

    // Keep foreground teardown bounded. If the backend does not become
    // waitable promptly, transfer ownership to a waiter so it is eventually
    // reaped without blocking terminal restoration.
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    thread::spawn(move || {
        let _ = child.wait();
    });
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn to_pty_size(size: TerminalSize) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{ProcessSpec, PtyProcess, TerminalSize};

    #[test]
    fn clamps_to_vt100_safe_minimum() {
        assert_eq!(TerminalSize::new(0, 0), TerminalSize::new(2, 2));
        assert_eq!(
            (TerminalSize::new(1, 1).cols, TerminalSize::new(1, 1).rows),
            (2, 2)
        );
    }

    #[test]
    fn termination_is_bounded_and_idempotent() {
        let mut spec = ProcessSpec::new("/bin/sh");
        spec.args = vec!["-c".to_string(), "sleep 30".to_string()];
        let mut process = PtyProcess::spawn(&spec, TerminalSize::new(20, 5)).unwrap();

        process.terminate();
        process.terminate();

        assert!(process.has_exited().unwrap());
    }
}
