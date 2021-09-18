use std::result::Result as StdResult;

use structopt::StructOpt;

use crate::{command::Command, context::Context, error::Error};

mod command;
mod context;
mod error;
mod procedure;

type Result<T> = StdResult<T, Error>;

fn main() {
    let wrapper = || -> Result<()> {
        let mut context = Context::load()?;

        match Command::from_args() {
            Command::Flash(proc) => context.run_procedure(proc)?,
        }
        Ok(())
    };

    match wrapper() {
        Ok(_) => (),
        Err(error) => eprintln!("hoc error: {}", error),
    }
}
