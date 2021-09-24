use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use serde::{de::DeserializeOwned, Serialize};

use crate::{context::ProcedureStep, error::Error, Result};

pub enum Halt<S> {
    Yield(S),
    Finish,
}

pub trait Procedure {
    type State: ProcedureState;
    const NAME: &'static str;

    fn rewind_state(&self) -> Option<<Self::State as ProcedureState>::Id>;
    fn run(&mut self, proc_step: &mut ProcedureStep) -> Result<Halt<Self::State>>;
}

pub trait ProcedureStateId: Clone + Copy + Hash + Eq + Ord
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
    type Id: ProcedureStateId;

    fn initial_state() -> Self;
    fn id(&self) -> Self::Id;
}
