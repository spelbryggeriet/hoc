use serde::{de::DeserializeOwned, Serialize};

use crate::Result;

pub enum Halt<S> {
    Yield(S),
    Finish,
}

pub trait Procedure {
    type State: ProcedureState;
    const NAME: &'static str;

    fn run(&mut self, state: Self::State) -> Result<Halt<Self::State>>;
}

pub trait ProcedureState: Serialize + DeserializeOwned {
    type Procedure: Procedure;

    const INITIAL_STATE: Self;

    fn description(&self) -> &'static str;

    #[allow(unused_variables)]
    fn needs_update(&self, procedure: &Self::Procedure) -> bool {
        false
    }
}
