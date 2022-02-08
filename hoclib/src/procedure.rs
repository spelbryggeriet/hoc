use std::{
    collections::HashMap,
    error::Error as StdError,
    path::{Path, PathBuf},
    str::FromStr,
};

use hoclog::error;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::context::dir_state::{self, DirectoryState};

#[macro_export]
macro_rules! halt {
    ($state:expr) => {
        Ok(::hoclib::Halt {
            persist: true,
            state: ::hoclib::HaltState::Halt($state),
        })
    };
}

#[macro_export]
macro_rules! finish {
    () => {
        Ok(::hoclib::Halt {
            persist: true,
            state: ::hoclib::HaltState::Finish,
        })
    };
}

#[macro_export]
macro_rules! transient_finish {
    () => {
        Ok(::hoclib::Halt {
            persist: false,
            state: ::hoclib::HaltState::Finish,
        })
    };
}

pub type Attributes = HashMap<String, Value>;

pub enum HaltState<S> {
    Halt(S),
    Finish,
}

pub struct Halt<S> {
    pub persist: bool,
    pub state: HaltState<S>,
}

pub trait Procedure {
    type State: ProcedureState;

    const NAME: &'static str;

    fn get_attributes(&self) -> Attributes {
        HashMap::default()
    }

    fn rewind_state(&self) -> Option<<Self::State as ProcedureState>::Id> {
        None
    }

    fn run(&mut self, step: &mut ProcedureStep) -> hoclog::Result<Halt<Self::State>>;
}

pub trait ProcedureStateId:
    Clone + Copy + Eq + Ord + FromStr<Err = Self::DeserializeError> + Into<&'static str>
where
    Self: Sized,
{
    type DeserializeError: 'static + StdError;

    fn description(&self) -> &'static str;

    fn as_str(self) -> &'static str {
        self.into()
    }

    fn parse<S: AsRef<str>>(input: S) -> Result<Self, Self::DeserializeError> {
        match Self::from_str(input.as_ref()) {
            Ok(id) => Ok(id),
            Err(err) => Err(err),
        }
    }
}

pub trait ProcedureState: Serialize + DeserializeOwned + Default {
    type Id: ProcedureStateId;

    fn id(&self) -> Self::Id;
}

#[derive(Debug, Error)]
pub enum ProcedureStepError {
    #[error("serde json: {0}")]
    SerdeJson(#[from] serde_json::Error),

    #[error("id: {0}")]
    Id(Box<dyn StdError>),

    #[error("dir state: {0}")]
    DirState(#[from] dir_state::DirectoryStateError),
}

impl From<ProcedureStepError> for hoclog::Error {
    fn from(err: ProcedureStepError) -> Self {
        error!(err.to_string()).unwrap_err()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProcedureStep {
    id: String,
    state: String,
    work_dir_state: DirectoryState,
}

impl ProcedureStep {
    pub fn new<S: ProcedureState, P: Into<PathBuf>>(
        state: &S,
        work_dir: P,
    ) -> Result<Self, ProcedureStepError> {
        Ok(Self {
            id: state.id().as_str().to_string(),
            state: serde_json::to_string(&state)?,
            work_dir_state: DirectoryState::new(work_dir)?,
        })
    }

    pub fn id<S: ProcedureState>(&self) -> Result<S::Id, ProcedureStepError> {
        S::Id::parse(&self.id).map_err(|e| ProcedureStepError::Id(Box::new(e)))
    }

    pub fn state<S: ProcedureState>(&self) -> Result<S, ProcedureStepError> {
        serde_json::from_str(&self.state).map_err(|e| ProcedureStepError::Id(Box::new(e)))
    }

    pub fn work_dir_state(&self) -> &DirectoryState {
        &self.work_dir_state
    }

    pub fn is_path_registered<P: AsRef<Path>>(
        &self,
        relative_path: P,
    ) -> Result<bool, ProcedureStepError> {
        Ok(self.work_dir_state.contains(relative_path)?)
    }

    pub fn register_file<P: AsRef<Path>>(
        &mut self,
        relative_path: P,
    ) -> Result<PathBuf, ProcedureStepError> {
        self.work_dir_state.register_file(&relative_path)?;
        let mut path = self.work_dir_state.root_path().to_path_buf();
        path.extend(relative_path.as_ref().iter());
        Ok(path)
    }

    pub fn register_dir<P: AsRef<Path>>(
        &mut self,
        relative_path: P,
    ) -> Result<PathBuf, ProcedureStepError> {
        self.work_dir_state.register_dir(&relative_path)?;
        let mut path = self.work_dir_state.root_path().to_path_buf();
        path.extend(relative_path.as_ref().iter());
        Ok(path)
    }

    pub fn unregister_path<P: AsRef<Path>>(
        &mut self,
        relative_path: P,
    ) -> Result<(), ProcedureStepError> {
        Ok(self.work_dir_state.unregister_path(&relative_path)?)
    }

    pub fn save_work_dir_changes(&mut self) -> Result<(), ProcedureStepError> {
        Ok(self.work_dir_state.update_states()?)
    }
}
