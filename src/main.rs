mod app;
mod backend;
mod clipboard;
mod terminal;
mod ui;
mod workbench;
mod workspace;

use anyhow::Result;

fn main() -> Result<()> {
    let command = match app::parse_args(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!("{}", app::SHORT_USAGE);
            std::process::exit(2);
        }
    };

    match command {
        app::Command::Run { mode, warning } => {
            if let Some(warning) = warning {
                eprintln!("warning: {warning}");
            }
            app::run(mode)
        }
        app::Command::Help => {
            println!("{}", app::HELP);
            Ok(())
        }
        app::Command::Version => {
            println!("{}", app::VERSION);
            Ok(())
        }
    }
}
