use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};

pub fn read_system() -> Result<String> {
    let output = Command::new("pbpaste")
        .output()
        .context("failed to run pbpaste")?;
    ensure!(
        output.status.success(),
        "pbpaste exited with {}",
        output.status
    );
    String::from_utf8(output.stdout).context("clipboard does not contain valid UTF-8")
}

pub fn write_system(contents: &str) -> Result<()> {
    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to run pbcopy")?;
    child
        .stdin
        .take()
        .context("failed to open pbcopy stdin")?
        .write_all(contents.as_bytes())
        .context("failed to write clipboard contents")?;
    let status = child.wait().context("failed to wait for pbcopy")?;
    ensure!(status.success(), "pbcopy exited with {status}");
    Ok(())
}
