use std::str::FromStr;

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
