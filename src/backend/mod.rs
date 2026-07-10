mod agent;
mod editor;
mod shell;

pub use agent::AgentBackend;
pub use editor::EditorBackend;
pub use shell::ShellBackend;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Editor,
    Agent,
    Shell,
}

pub trait Backend {
    fn kind(&self) -> BackendKind;
    fn name(&self) -> &str;
}
