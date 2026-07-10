#![allow(dead_code, unused_imports)]

mod app;
mod backend;
mod terminal;
mod ui;
mod workbench;
mod workspace;

use anyhow::Result;

fn main() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("nvim") | Some("--nvim") => app::run_nvim_spike(),
        Some("pi") | Some("--pi") => app::run_pi_spike(),
        Some("multi") | Some("--multi") => app::run_multi_spike(),
        _ => app::run_shell_spike(),
    }
}
