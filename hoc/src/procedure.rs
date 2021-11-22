use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;

use crate::{context::dir_state::DirectoryState, error::Error, Result};

pub type Attributes = HashMap<String, Value>;

pub enum HaltState<S> {
    Yield(S),
    Finish,
}

pub struct Halt<S> {
    pub persist: bool,
    pub state: HaltState<S>,
}

impl<S> Halt<S> {
    pub fn persistent_yield(state: S) -> Self {
        Self {
            persist: true,
            state: HaltState::Yield(state),
        }
    }

    pub fn persistent_finish() -> Self {
        Self {
            persist: true,
            state: HaltState::Finish,
        }
    }

    pub fn transient_yield(state: S) -> Self {
        Self {
            persist: false,
            state: HaltState::Yield(state),
        }
    }

    pub fn transient_finish() -> Self {
        Self {
            persist: false,
            state: HaltState::Finish,
        }
    }
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

    fn run(&mut self, step: &mut ProcedureStep) -> Result<Halt<Self::State>>;
}

pub trait ProcedureStateId:
    Clone + Copy + Eq + Ord + FromStr<Err = Self::DeserializeError> + Into<&'static str>
where
    Self: Sized,
{
    type DeserializeError: Into<Error>;

    fn description(&self) -> &'static str;

    fn as_str(self) -> &'static str {
        self.into()
    }

    fn parse<S: AsRef<str>>(input: S) -> Result<Self> {
        match Self::from_str(input.as_ref()) {
            Ok(id) => Ok(id),
            Err(err) => Err(err.into()),
        }
    }
}

pub trait ProcedureState: Serialize + DeserializeOwned + Default {
    type Id: ProcedureStateId;

    fn id(&self) -> Self::Id;
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProcedureStep {
    id: String,
    state: String,
    work_dir_state: DirectoryState,
}

impl ProcedureStep {
    pub fn new<S: ProcedureState, P: Into<PathBuf>>(state: &S, work_dir: P) -> Result<Self> {
        Ok(Self {
            id: state.id().as_str().to_string(),
            state: serde_json::to_string(&state)?,
            work_dir_state: DirectoryState::new(work_dir)?,
        })
    }

    pub fn id<S: ProcedureState>(&self) -> Result<S::Id> {
        Ok(S::Id::parse(&self.id)?)
    }

    pub fn state<S: ProcedureState>(&self) -> Result<S> {
        Ok(serde_json::from_str(&self.state)?)
    }

    pub fn work_dir_state(&self) -> &DirectoryState {
        &self.work_dir_state
    }

    pub fn get_real_path<P: AsRef<Path>>(&self, relative_path: P) -> PathBuf {
        let mut real_path = self.work_dir_state.root_path().to_path_buf();
        real_path.extend(relative_path.as_ref().iter());

        real_path
    }

    pub fn register_path<P: AsRef<Path>>(&mut self, relative_path: P) -> Result<PathBuf> {
        self.work_dir_state.register_path(&relative_path)?;
        let mut path = self.work_dir_state.root_path().to_path_buf();
        path.extend(relative_path.as_ref().iter());
        Ok(path)
    }

    pub fn unregister_path<P: AsRef<Path>>(&mut self, relative_path: P) -> Result<()> {
        self.work_dir_state.unregister_path(&relative_path)?;
        Ok(())
    }

    pub fn save_work_dir_changes(&mut self) -> Result<()> {
        self.work_dir_state.update_states()
    }
}
