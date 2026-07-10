use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
}

impl TerminalSize {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols: cols.max(1),
            rows: rows.max(1),
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
    child: Box<dyn portable_pty::Child + Send>,
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

        let mut reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone PTY reader")?;
        let writer = pair
            .master
            .take_writer()
            .context("failed to take PTY writer")?;

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
            child,
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
        self.child
            .try_wait()
            .map(|status| status.is_some())
            .context("failed to poll PTY child")
    }

    pub fn terminate(&mut self) {
        let _ = self.child.kill();
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
