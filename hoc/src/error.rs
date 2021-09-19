use std::{env::VarError, io};

use thiserror::Error;

use crate::context::Context;

fn get_context_display_text() -> String {
    Context::get_context_dir()
        .map(|mut cd| {
            cd.push(Context::CONTEXT_FILE_NAME);
            cd.to_string_lossy().into_owned()
        })
        .unwrap_or_else(|_| "context file".to_owned())
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to retrieve value for environment variable `{name}`: {source}")]
    Environment {
        name: &'static str,
        source: VarError,
    },

    #[error(
        "{} could not be serialized/deserialized: {0}",
        get_context_display_text()
    )]
    ContextSerde(#[from] serde_yaml::Error),

    #[error("procedure state could not be serialized/deserialized: {0}")]
    ProcedureStateSerde(#[from] serde_json::Error),

    #[error(transparent)]
    LogError(#[from] hoclog::Error),

    #[error(transparent)]
    Io(#[from] io::Error),
}

impl Error {
    pub fn environment(name: &'static str, source: VarError) -> Self {
        Self::Environment { name, source }
    }
}
