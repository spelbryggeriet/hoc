use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use serde::{de::DeserializeOwned, Serialize};

use crate::{error::Error, Result};

pub enum Halt<S> {
    Yield(S),
    Finish,
}

pub trait Procedure {
    type State: ProcedureState;
    const NAME: &'static str;

    fn run(&mut self, state: Self::State) -> Result<Halt<Self::State>>;
}

pub trait ProcedureStateId: Hash + Eq + Ord
where
    Self: Sized,
{
    type MemberIter: Iterator<Item = Self>;

    fn members() -> Self::MemberIter;
    fn description(&self) -> &'static str;

    fn to_hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }

    fn from_hash(hash: u64) -> Result<Self> {
        Self::members()
            .find(|s| s.to_hash() == hash)
            .ok_or(Error::InvalidProcedureStateIdHash(hash))
    }
}

pub trait ProcedureState: Serialize + DeserializeOwned {
    type Procedure: Procedure;
    type Id: ProcedureStateId;

    fn initial_state() -> Self;
    fn id(&self) -> Self::Id;

    #[allow(unused_variables)]
    fn needs_update(&self, procedure: &Self::Procedure) -> Result<Option<UpdateInfo<Self::Id>>> {
        Ok(None)
    }
}

pub struct UpdateInfo<I> {
    pub state_id: I,
    pub description: String,
    pub user_choice: bool,
}

impl<I> UpdateInfo<I>
where
    I: ProcedureStateId,
{
    pub fn user_update(state_id: I, description: impl ToString) -> Self {
        Self {
            state_id,
            description: description.to_string(),
            user_choice: true,
        }
    }

    pub fn invalid_state(state_id: I, cause: impl ToString) -> Self {
        Self {
            state_id,
            description: cause.to_string(),
            user_choice: false,
        }
    }
}
