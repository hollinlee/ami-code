use super::{Backend, BackendKind};

#[derive(Debug, Clone)]
pub struct EditorBackend {
    name: String,
}

impl EditorBackend {
    pub fn nvim() -> Self {
        Self {
            name: "nvim".to_string(),
        }
    }
}

impl Backend for EditorBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Editor
    }

    fn name(&self) -> &str {
        &self.name
    }
}
