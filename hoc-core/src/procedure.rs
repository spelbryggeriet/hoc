use std::{
    error::Error as StdError,
    hash::{Hash, Hasher},
    ops::{Deref, DerefMut},
    str::FromStr,
};

use hoc_log::error;
use indexmap::IndexMap;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;

use crate::{kv::WriteStore, process};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attributes(IndexMap<String, String>);

impl Attributes {
    pub fn new() -> Self {
        Self(IndexMap::new())
    }
}

impl Hash for Attributes {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for (key, value) in self.0.iter() {
            state.write(key.as_bytes());
            state.write(value.as_bytes());
        }
    }
}

impl Deref for Attributes {
    type Target = IndexMap<String, String>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Attributes {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
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

    fn get_attributes(&self) -> Attributes {
        Attributes::default()
    }

    fn rewind_state(&self) -> Option<<Self::State as State>::Id> {
        None
    }

    fn run(
        &mut self,
        state: Self::State,
        registry: &impl WriteStore,
    ) -> hoc_log::Result<Halt<Self::State>>;
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
}

impl From<Error> for hoc_log::Error {
    fn from(err: Error) -> Self {
        error!("{err}").unwrap_err()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Step {
    state: String,
    id: String,
}

impl Step {
    pub fn new<P: Procedure>() -> Result<Self, Error> {
        let state = P::State::default();
        Ok(Self {
            id: state.id().as_str().to_string(),
            state: serde_json::to_string(&state)?,
        })
    }

    pub fn from_state<S: State>(state: &S) -> Result<Self, Error> {
        Ok(Self {
            id: state.id().as_str().to_string(),
            state: serde_json::to_string(state)?,
        })
    }

    pub fn id<S: State>(&self) -> Result<S::Id, Error> {
        S::Id::parse(&self.id).map_err(|e| Error::Id(Box::new(e)))
    }

    pub fn state<S: State>(&self) -> Result<S, Error> {
        serde_json::from_str(&self.state).map_err(|e| Error::Id(Box::new(e)))
    }
}
