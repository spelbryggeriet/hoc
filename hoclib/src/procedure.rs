use std::{error::Error as StdError, hash::Hash, str::FromStr};

use hoclog::error;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;

use crate::{dir_state, process, Context, DirState};

#[macro_export]
macro_rules! attributes {
    ($($k:expr => $v:expr),* $(,)?) => {{
        let mut map = Vec::new();
        $(map.push($crate::procedure::Attribute { key: ($k).into(), value: ($v).into() });)*
        map
    }};
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct Attribute {
    pub key: String,
    pub value: String,
}

pub enum HaltState<S> {
    Halt(S),
    Finish,
}

pub struct Halt<S> {
    pub persist: bool,
    pub state: HaltState<S>,
}

pub trait Procedure: Sized {
    type State: State;

    const NAME: &'static str;

    fn get_attributes(&self) -> Vec<Attribute> {
        Vec::default()
    }

    fn rewind_state(&self) -> Option<<Self::State as State>::Id> {
        None
    }

    fn run(&mut self, step: &mut Step) -> hoclog::Result<Halt<Self::State>>;
}

pub trait Id:
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

pub trait State: Serialize + DeserializeOwned + Default {
    type Procedure: Procedure;
    type Id: Id;

    fn id(&self) -> Self::Id;
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("serde json: {0}")]
    SerdeJson(#[from] serde_json::Error),

    #[error("id: {0}")]
    Id(Box<dyn StdError>),

    #[error("process: {0}")]
    Process(#[from] process::Error),

    #[error("directory state: {0}")]
    DirState(#[from] dir_state::Error),
}

impl From<Error> for hoclog::Error {
    fn from(err: Error) -> Self {
        error!(err.to_string()).unwrap_err()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Step {
    state: String,
    id: String,
    work_dir_state: DirState,
}

impl Step {
    pub fn from_procedure<P: Procedure>(procedure: &P) -> Result<Self, Error> {
        let state = P::State::default();
        Ok(Self {
            id: state.id().as_str().to_string(),
            state: serde_json::to_string(&state)?,
            work_dir_state: DirState::empty(Context::get_work_dir::<P>(
                &procedure.get_attributes(),
            ))?,
        })
    }

    pub fn from_states<S: State>(state: &S, work_dir_state: DirState) -> Result<Self, Error> {
        Ok(Self {
            id: state.id().as_str().to_string(),
            state: serde_json::to_string(state)?,
            work_dir_state,
        })
    }

    pub fn id<S: State>(&self) -> Result<S::Id, Error> {
        S::Id::parse(&self.id).map_err(|e| Error::Id(Box::new(e)))
    }

    pub fn state<S: State>(&self) -> Result<S, Error> {
        serde_json::from_str(&self.state).map_err(|e| Error::Id(Box::new(e)))
    }

    pub fn work_dir_state(&self) -> &DirState {
        &self.work_dir_state
    }

    pub fn work_dir_state_mut(&mut self) -> &mut DirState {
        &mut self.work_dir_state
    }
}
