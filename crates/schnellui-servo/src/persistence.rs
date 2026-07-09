use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::{BrowserError, BrowserSessionState};

#[derive(Clone, Debug)]
pub struct BrowserStateStore {
    path: PathBuf,
}

impl BrowserStateStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<BrowserSessionState, StateStoreError> {
        let bytes = fs::read(&self.path).map_err(StateStoreError::Read)?;
        let state = serde_json::from_slice::<BrowserSessionState>(&bytes)
            .map_err(StateStoreError::Decode)?;
        state.validate().map_err(StateStoreError::Invalid)?;
        Ok(state)
    }

    pub fn load_or_default(&self) -> Result<BrowserSessionState, StateStoreError> {
        match self.load() {
            Ok(state) => Ok(state),
            Err(StateStoreError::Read(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(BrowserSessionState::default())
            }
            Err(error) => Err(error),
        }
    }

    /// Atomically replaces the state file and fsyncs both file and parent directory.
    pub fn save(&self, state: &BrowserSessionState) -> Result<(), StateStoreError> {
        state.validate().map_err(StateStoreError::Invalid)?;
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(StateStoreError::Write)?;
        let bytes = serde_json::to_vec_pretty(state).map_err(StateStoreError::Encode)?;
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("browser-state.json");
        let temporary = parent.join(format!(".{file_name}.tmp-{}", std::process::id()));
        let result = (|| {
            let mut file = fs::File::create(&temporary).map_err(StateStoreError::Write)?;
            file.write_all(&bytes).map_err(StateStoreError::Write)?;
            file.sync_all().map_err(StateStoreError::Write)?;
            fs::rename(&temporary, &self.path).map_err(StateStoreError::Write)?;
            fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(StateStoreError::Write)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StateStoreError {
    #[error("failed to read browser state: {0}")]
    Read(std::io::Error),
    #[error("failed to write browser state: {0}")]
    Write(std::io::Error),
    #[error("failed to decode browser state: {0}")]
    Decode(serde_json::Error),
    #[error("failed to encode browser state: {0}")]
    Encode(serde_json::Error),
    #[error("invalid browser state: {0}")]
    Invalid(BrowserError),
}
