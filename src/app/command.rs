use std::fmt;

pub const SHORT_USAGE: &str = "Usage: ami-code [shell|nvim|pi]";

pub const HELP: &str = concat!(
    "ami-code ",
    env!("CARGO_PKG_VERSION"),
    "\n\n",
    "A terminal workbench for coding.\n\n",
    "Usage: ami-code [shell|nvim|pi]\n\n",
    "Modes:\n",
    "  shell    Run a single shell session\n",
    "  nvim     Run a single Neovim session\n",
    "  pi       Run a single Pi session\n\n",
    "With no mode, ami-code starts the Workbench.\n\n",
    "Options:\n",
    "  -h, --help       Print help\n",
    "  -V, --version    Print version\n",
);

pub const VERSION: &str = concat!("ami-code ", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    Shell,
    Nvim,
    Pi,
    Workbench,
}

impl LaunchMode {
    pub fn is_workbench(self) -> bool {
        self == Self::Workbench
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run {
        mode: LaunchMode,
        warning: Option<&'static str>,
    },
    Help,
    Version,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(String);

impl ParseError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Parses arguments after the executable name.
///
/// The alpha CLI deliberately accepts exactly zero or one argument: backend
/// arguments are not passed through.
pub fn parse_args<I, S>(args: I) -> Result<Command, ParseError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args
        .into_iter()
        .map(|argument| argument.as_ref().to_owned())
        .collect();

    let Some(first) = args.first() else {
        return Ok(Command::Run {
            mode: LaunchMode::Workbench,
            warning: None,
        });
    };

    if args.len() > 1 {
        let second = &args[1];
        if is_mode(first) && is_mode(second) {
            return Err(ParseError::new(format!(
                "mode specified more than once: `{first}` and `{second}`"
            )));
        }
        return Err(ParseError::new(format!(
            "unexpected extra argument `{second}`"
        )));
    }

    let (mode, warning) = match first.as_str() {
        "shell" => (LaunchMode::Shell, None),
        "nvim" => (LaunchMode::Nvim, None),
        "pi" => (LaunchMode::Pi, None),
        "multi" => (
            LaunchMode::Workbench,
            Some("`multi` is deprecated; use `ami-code` (with no mode)"),
        ),
        "--multi" => (
            LaunchMode::Workbench,
            Some("`--multi` is deprecated; use `ami-code` (with no mode)"),
        ),
        "--nvim" => (
            LaunchMode::Nvim,
            Some("`--nvim` is deprecated; use `ami-code nvim`"),
        ),
        "--pi" => (
            LaunchMode::Pi,
            Some("`--pi` is deprecated; use `ami-code pi`"),
        ),
        "-h" | "--help" => return Ok(Command::Help),
        "-V" | "--version" => return Ok(Command::Version),
        argument if argument.starts_with('-') => {
            return Err(ParseError::new(format!("unknown option `{argument}`")));
        }
        argument => return Err(ParseError::new(format!("unknown mode `{argument}`"))),
    };

    Ok(Command::Run { mode, warning })
}

fn is_mode(argument: &str) -> bool {
    matches!(
        argument,
        "shell" | "nvim" | "pi" | "multi" | "--multi" | "--nvim" | "--pi"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_and_canonical_modes() {
        let cases = [
            (&[][..], LaunchMode::Workbench),
            (&["shell"][..], LaunchMode::Shell),
            (&["nvim"][..], LaunchMode::Nvim),
            (&["pi"][..], LaunchMode::Pi),
        ];

        for (args, expected_mode) in cases {
            assert_eq!(
                parse_args(args),
                Ok(Command::Run {
                    mode: expected_mode,
                    warning: None,
                }),
                "arguments: {args:?}"
            );
        }
    }

    #[test]
    fn parses_every_compatibility_alias_with_a_warning() {
        let cases = [
            (
                "multi",
                LaunchMode::Workbench,
                "`multi` is deprecated; use `ami-code` (with no mode)",
            ),
            (
                "--multi",
                LaunchMode::Workbench,
                "`--multi` is deprecated; use `ami-code` (with no mode)",
            ),
            (
                "--nvim",
                LaunchMode::Nvim,
                "`--nvim` is deprecated; use `ami-code nvim`",
            ),
            (
                "--pi",
                LaunchMode::Pi,
                "`--pi` is deprecated; use `ami-code pi`",
            ),
        ];

        for (alias, expected_mode, expected_warning) in cases {
            assert_eq!(
                parse_args([alias]),
                Ok(Command::Run {
                    mode: expected_mode,
                    warning: Some(expected_warning),
                }),
                "alias: {alias}"
            );
        }
    }

    #[test]
    fn parses_help_and_version_flags() {
        let cases = [
            ("-h", Command::Help),
            ("--help", Command::Help),
            ("-V", Command::Version),
            ("--version", Command::Version),
        ];

        for (argument, expected) in cases {
            assert_eq!(parse_args([argument]), Ok(expected), "flag: {argument}");
        }
    }

    #[test]
    fn rejects_unknown_modes_and_options() {
        let cases = [
            ("wat", "unknown mode `wat`"),
            ("--wat", "unknown option `--wat`"),
            ("-x", "unknown option `-x`"),
        ];

        for (argument, expected) in cases {
            assert_eq!(
                parse_args([argument]).unwrap_err().to_string(),
                expected,
                "argument: {argument}"
            );
        }
    }

    #[test]
    fn rejects_duplicate_modes_and_extra_arguments() {
        let cases = [
            (
                &["shell", "pi"][..],
                "mode specified more than once: `shell` and `pi`",
            ),
            (
                &["multi", "--nvim"][..],
                "mode specified more than once: `multi` and `--nvim`",
            ),
            (
                &["nvim", "file.rs"][..],
                "unexpected extra argument `file.rs`",
            ),
            (
                &["--help", "shell"][..],
                "unexpected extra argument `shell`",
            ),
            (
                &["--version", "extra"][..],
                "unexpected extra argument `extra`",
            ),
            (
                &["shell", "pi", "extra"][..],
                "mode specified more than once: `shell` and `pi`",
            ),
        ];

        for (args, expected) in cases {
            assert_eq!(
                parse_args(args).unwrap_err().to_string(),
                expected,
                "arguments: {args:?}"
            );
        }
    }
}
