use std::{
    collections::{hash_map::Iter, HashMap},
    error::Error as StdError,
    hash::{Hash, Hasher},
    mem,
    str::FromStr,
};

use hoclog::error;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::{dir_state, process, Context, DirState};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attributes(HashMap<String, Value>);

impl Hash for Attributes {
    fn hash<H: Hasher>(&self, state: &mut H) {
        fn hash_value<H: Hasher>(v: &Value, state: &mut H) {
            match v {
                Value::Null => ().hash(state),
                Value::Bool(b) => b.hash(state),
                Value::Number(n) => {
                    if let Some(f) = n.as_f64() {
                        let bits: u64 = unsafe { mem::transmute(f) };
                        bits.hash(state)
                    } else if let Some(i) = n.as_i64() {
                        i.hash(state)
                    } else {
                        n.as_u64().unwrap().hash(state)
                    }
                }
                Value::String(s) => s.hash(state),
                Value::Array(vs) => {
                    for v in vs {
                        hash_value(v, state);
                    }
                }
                Value::Object(vs) => {
                    for (k, v) in vs.iter() {
                        k.hash(state);
                        hash_value(v, state);
                    }
                }
            }
        }

        for (key, value) in self.0.iter() {
            key.hash(state);
            hash_value(value, state);
        }
    }
}

impl Attributes {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> Iter<String, Value> {
        self.0.iter()
    }

    pub fn insert(&mut self, k: String, v: Value) -> Option<Value> {
        self.0.insert(k, v)
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
            work_dir_state: DirState::empty(Context::get_work_dir(procedure))?,
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
