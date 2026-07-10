use super::{Backend, BackendKind};

#[derive(Debug, Clone)]
pub struct ShellBackend {
    name: String,
}

impl ShellBackend {
    pub fn system_default() -> Self {
        Self {
            name: std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string()),
        }
    }
}

impl Backend for ShellBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Shell
    }

    fn name(&self) -> &str {
        &self.name
    }
}
