use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn discover(start: impl AsRef<Path>) -> Result<Self> {
        let root = start.as_ref().canonicalize().with_context(|| {
            format!(
                "failed to resolve workspace root: {}",
                start.as_ref().display()
            )
        })?;

        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}
