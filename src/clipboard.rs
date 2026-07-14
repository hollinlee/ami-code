use std::process::Command;

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
