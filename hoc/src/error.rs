use std::{env::VarError, io};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to retrieve value for environment variable `{name}`: {source}")]
    Environment {
        name: &'static str,
        source: VarError,
    },

    #[error("context could not be serialized/deserialized: {0}")]
    ContextSerde(#[from] serde_yaml::Error),

    #[error("procedure state could not be serialized/deserialized: {0}")]
    ProcedureStateSerde(#[from] serde_json::Error),

    #[error(transparent)]
    Io(#[from] io::Error),
}

impl Error {
    pub fn environment(name: &'static str, source: VarError) -> Self {
        Self::Environment { name, source }
    }
}
