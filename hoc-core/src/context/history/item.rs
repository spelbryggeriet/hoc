use std::{io, path::PathBuf};

use hoc_log::error;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    procedure::{self, Procedure, State, Step},
    process,
};

#[derive(Debug, Error)]
pub enum Error {
    #[error("procedure step: {0}")]
    ProcedureStep(#[from] procedure::Error),

    #[error("process: {0}")]
    Process(#[from] process::Error),

    #[error("io: {0}")]
    Io(#[from] io::Error),
}

impl From<Error> for hoc_log::Error {
    fn from(err: Error) -> Self {
        error!("{err}").unwrap_err()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Item {
    registry_keys: Vec<PathBuf>,
    #[serde(rename = "completed_steps")]
    completed: Vec<Step>,
    #[serde(rename = "current_step")]
    current: Option<Step>,
}

impl Item {
    pub fn new<P: Procedure>() -> Result<Self, Error> {
        Ok(Self {
            registry_keys: Vec::new(),
            completed: Vec::new(),
            current: Some(Step::new::<P>()?),
        })
    }

    pub fn registry_keys(&self) -> &[PathBuf] {
        &self.registry_keys
    }

    pub fn completed(&self) -> &[Step] {
        &self.completed
    }

    pub fn current(&self) -> Option<&Step> {
        self.current.as_ref()
    }

    pub fn current_mut(&mut self) -> Option<&mut Step> {
        self.current.as_mut()
    }

    pub fn is_complete(&self) -> bool {
        self.current().is_none()
    }

    pub fn next<S: State>(
        &mut self,
        state: &Option<S>,
        registry_keys: Vec<PathBuf>,
    ) -> Result<(), Error> {
        self.registry_keys = registry_keys;

        if let Some(state) = state {
            if let Some(completed_step) = self.current.replace(Step::from_state(state)?) {
                self.completed.push(completed_step);
            }
        } else if let Some(completed_step) = self.current.take() {
            self.completed.push(completed_step);
        }

        Ok(())
    }
}
