mod app;
mod backend;
mod terminal;
mod ui;
mod workbench;
mod workspace;

use anyhow::Result;

fn main() -> Result<()> {
    let mode = app::LaunchMode::from_arg(std::env::args().nth(1).as_deref());
    app::run(mode)
}
