use std::result::Result as StdResult;

pub use context::{
    dir_state::{DirectoryState, FileStateDiff},
    Cache, Context,
};
pub use error::Error;
pub use procedure::{
    Attributes, Halt, HaltState, Procedure, ProcedureState, ProcedureStateId, ProcedureStep,
};
pub use process::{reset_sudo_privileges, ssh, Process};

mod context;
mod error;
mod procedure;
mod process;

pub type Result<T> = StdResult<T, Error>;
