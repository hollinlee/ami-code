use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system};
use tempfile::TempDir;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(8);
const OUTPUT_CHANNEL_CAPACITY: usize = 64;
const DRAIN_CHUNK_BUDGET: usize = 64;
const RAW_TRANSCRIPT_LIMIT: usize = 256 * 1024;

pub struct PtyHarness {
    master: Box<dyn MasterPty + Send>,
    writer: Option<Box<dyn Write + Send>>,
    child: Option<Box<dyn Child + Send>>,
    rx: Receiver<Vec<u8>>,
    parser: vt100::Parser,
    raw: Vec<u8>,
    timeout: Duration,
    fixture: TempDir,
}

impl PtyHarness {
    pub fn spawn(args: &[&str], cols: u16, rows: u16) -> Self {
        let fixture = TempDir::new().expect("create PTY fixture");
        let workspace = fixture.path().join("workspace");
        let home = fixture.path().join("home");
        let state = fixture.path().join("state");
        fs::create_dir_all(&workspace).expect("create workspace");
        fs::create_dir_all(&home).expect("create HOME");
        let shell = create_fixture_shell(fixture.path());

        let pair = native_pty_system()
            .openpty(pty_size(cols, rows))
            .expect("open test PTY");
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_ami-code"));
        command.args(args);
        command.cwd(&workspace);
        command.env_clear();
        command.env("HOME", &home);
        command.env("AMI_CODE_STATE_DIR", &state);
        command.env("SHELL", &shell);
        command.env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
        command.env("TERM", "xterm-256color");
        command.env("LC_ALL", "C");
        command.env("PI_SKIP_VERSION_CHECK", "1");
        command.env("PI_TELEMETRY", "0");
        command.env("AMI_CODE_TEST_CHILD_PID", fixture.path().join("child.pid"));
        command.env(
            "AMI_CODE_TEST_CHILD_INPUT",
            fixture.path().join("child-input"),
        );

        let child = pair
            .slave
            .spawn_command(command)
            .expect("spawn ami-code in test PTY");
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().expect("clone PTY reader");
        let writer = pair.master.take_writer().expect("take PTY writer");
        let (tx, rx) = mpsc::sync_channel(OUTPUT_CHANNEL_CAPACITY);
        thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) if tx.send(buffer[..read].to_vec()).is_err() => break,
                    Ok(_) => {}
                }
            }
        });

        Self {
            master: pair.master,
            writer: Some(writer),
            child: Some(child),
            rx,
            parser: vt100::Parser::new(rows.max(1), cols.max(1), 0),
            raw: Vec::new(),
            timeout: DEFAULT_TIMEOUT,
            fixture,
        }
    }

    pub fn expect_screen(&mut self, label: &str, predicate: impl Fn(&str) -> bool) {
        let deadline = Instant::now() + self.timeout;
        loop {
            self.drain_available();
            let contents = self.parser.screen().contents();
            if predicate(&contents) {
                return;
            }
            if self.child_exited() {
                self.fail(label, "process exited before screen matched");
            }
            let now = Instant::now();
            if now >= deadline {
                self.fail(label, "timed out waiting for screen");
            }
            match self
                .rx
                .recv_timeout((deadline - now).min(Duration::from_millis(50)))
            {
                Ok(bytes) => self.consume(bytes),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.fail(label, "PTY output closed before screen matched");
                }
            }
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.master
            .resize(pty_size(cols, rows))
            .expect("resize test PTY");
        self.parser.screen_mut().set_size(rows.max(1), cols.max(1));
    }

    pub fn send(&mut self, bytes: &[u8]) {
        let writer = self.writer.as_mut().expect("PTY input is open");
        writer.write_all(bytes).expect("write test PTY input");
        writer.flush().expect("flush test PTY input");
    }

    pub fn wait_for_exit(&mut self) -> ExitStatus {
        let deadline = Instant::now() + self.timeout;
        loop {
            self.drain_available();
            if let Some(status) = self
                .child
                .as_mut()
                .expect("child is present")
                .try_wait()
                .expect("poll ami-code child")
            {
                self.drain_after_exit();
                return status;
            }
            if Instant::now() >= deadline {
                self.fail("clean exit", "timed out waiting for process exit");
            }
            match self.rx.recv_timeout(Duration::from_millis(25)) {
                Ok(bytes) => self.consume(bytes),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {}
            }
        }
    }

    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    pub fn child_pid_path(&self) -> PathBuf {
        self.fixture.path().join("child.pid")
    }

    pub fn child_input_path(&self) -> PathBuf {
        self.fixture.path().join("child-input")
    }

    fn consume(&mut self, bytes: Vec<u8>) {
        self.parser.process(&bytes);
        retain_raw_tail(&mut self.raw, &bytes);
    }

    fn drain_available(&mut self) {
        for _ in 0..DRAIN_CHUNK_BUDGET {
            let Ok(bytes) = self.rx.try_recv() else {
                break;
            };
            self.consume(bytes);
        }
    }

    fn drain_after_exit(&mut self) {
        let deadline = Instant::now() + Duration::from_millis(250);
        loop {
            self.drain_available();
            if Instant::now() >= deadline {
                break;
            }
            match self.rx.recv_timeout(Duration::from_millis(10)) {
                Ok(bytes) => self.consume(bytes),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    fn child_exited(&mut self) -> bool {
        self.child
            .as_mut()
            .and_then(|child| child.try_wait().ok().flatten())
            .is_some()
    }

    fn fail(&mut self, label: &str, reason: &str) -> ! {
        self.drain_available();
        let screen = self.parser.screen().contents();
        let tail_start = self.raw.len().saturating_sub(2000);
        let tail = String::from_utf8_lossy(&self.raw[tail_start..]);
        panic!("{label}: {reason}\nscreen:\n{screen}\nraw tail:\n{tail:?}");
    }
}

impl Drop for PtyHarness {
    fn drop(&mut self) {
        self.writer.take();
        let Some(mut child) = self.child.take() else {
            return;
        };
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
            let deadline = Instant::now() + Duration::from_millis(500);
            while Instant::now() < deadline {
                if child.try_wait().ok().flatten().is_some() {
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

fn retain_raw_tail(raw: &mut Vec<u8>, bytes: &[u8]) {
    if bytes.len() >= RAW_TRANSCRIPT_LIMIT {
        raw.clear();
        raw.extend_from_slice(&bytes[bytes.len() - RAW_TRANSCRIPT_LIMIT..]);
        return;
    }
    let overflow = raw
        .len()
        .saturating_add(bytes.len())
        .saturating_sub(RAW_TRANSCRIPT_LIMIT);
    if overflow > 0 {
        raw.drain(..overflow);
    }
    raw.extend_from_slice(bytes);
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn create_fixture_shell(root: &Path) -> PathBuf {
    let path = root.join("fixture-shell");
    fs::write(
        &path,
        "#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$AMI_CODE_TEST_CHILD_PID\"\ntrap 'exit 0' HUP INT TERM\nwhile IFS= read -r line; do printf '<%s>\\n' \"$line\" >> \"$AMI_CODE_TEST_CHILD_INPUT\"; done\n",
    )
    .expect("write fixture shell");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .expect("make fixture shell executable");
    }
    path
}

#[cfg(test)]
mod tests {
    use super::{RAW_TRANSCRIPT_LIMIT, retain_raw_tail};

    #[test]
    fn transcript_retention_is_bounded_and_keeps_the_latest_output() {
        let mut raw = Vec::new();
        for byte in 0_u8..=255 {
            retain_raw_tail(&mut raw, &vec![byte; 4096]);
            assert!(raw.len() <= RAW_TRANSCRIPT_LIMIT);
        }
        assert_eq!(raw.len(), RAW_TRANSCRIPT_LIMIT);
        assert!(raw.ends_with(&vec![255; 4096]));

        let oversized = (0..RAW_TRANSCRIPT_LIMIT + 17)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        retain_raw_tail(&mut raw, &oversized);
        assert_eq!(raw, oversized[17..]);
    }
}
