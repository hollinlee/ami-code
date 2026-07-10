#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceConfig {
    pub name: Option<String>,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self { name: None }
    }
}
