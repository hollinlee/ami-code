#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    Shell,
    Nvim,
    Pi,
    Workbench,
}

impl LaunchMode {
    pub fn from_arg(arg: Option<&str>) -> Self {
        match arg {
            Some("nvim" | "--nvim") => Self::Nvim,
            Some("pi" | "--pi") => Self::Pi,
            Some("multi" | "--multi") => Self::Workbench,
            _ => Self::Shell,
        }
    }

    pub fn is_workbench(self) -> bool {
        self == Self::Workbench
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_cli_compatibility() {
        assert_eq!(LaunchMode::from_arg(None), LaunchMode::Shell);
        assert_eq!(LaunchMode::from_arg(Some("nvim")), LaunchMode::Nvim);
        assert_eq!(LaunchMode::from_arg(Some("--nvim")), LaunchMode::Nvim);
        assert_eq!(LaunchMode::from_arg(Some("pi")), LaunchMode::Pi);
        assert_eq!(LaunchMode::from_arg(Some("--pi")), LaunchMode::Pi);
        assert_eq!(LaunchMode::from_arg(Some("multi")), LaunchMode::Workbench);
        assert_eq!(LaunchMode::from_arg(Some("--multi")), LaunchMode::Workbench);
    }
}
