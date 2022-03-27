use std::io;

use hoclog::error;
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

impl From<Error> for hoclog::Error {
    fn from(err: Error) -> Self {
        error!("{err}").unwrap_err()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Item {
    #[serde(rename = "completed_steps")]
    completed: Vec<Step>,
    #[serde(rename = "current_step")]
    current: Option<Step>,
}

impl Item {
    pub fn new<P: Procedure>() -> Result<Self, Error> {
        Ok(Self {
            completed: Vec::new(),
            current: Some(Step::new::<P>()?),
        })
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

    pub fn next<S: State>(&mut self, state: &Option<S>) -> Result<(), Error> {
        if let Some(state) = state {
            if let Some(completed_step) = self.current.replace(Step::from_state(state)?) {
                self.completed.push(completed_step);
            }
        } else if let Some(completed_step) = self.current.take() {
            self.completed.push(completed_step);
        }

        Ok(())
    }

    pub fn invalidate<S: State>(&mut self, id: S::Id) -> Result<(), Error> {
        for (index, step) in self.completed.iter().enumerate() {
            if id == step.id::<S>()? {
                self.completed.truncate(index + 1);
                self.current.replace(self.completed.remove(index));
                break;
            }
        }

        Ok(())
    }
}
