use super::{Backend, BackendKind};

#[derive(Debug, Clone)]
pub struct AgentBackend {
    name: String,
}

impl AgentBackend {
    pub fn pi() -> Self {
        Self {
            name: "pi".to_string(),
        }
    }
}

impl Backend for AgentBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Agent
    }

    fn name(&self) -> &str {
        &self.name
    }
}
