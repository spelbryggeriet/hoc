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
    fn description(state_id: Self::Id) -> &'static str;
    fn id(&self) -> Self::Id;

    #[allow(unused_variables)]
    fn needs_update(&self, procedure: &Self::Procedure) -> Result<Option<Self::Id>> {
        Ok(None)
    }
}
