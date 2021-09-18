use std::{fs::OpenOptions, result::Result as StdResult};

use context::ProcedureCache;
use hoclog::status;
use procedure::{Procedure, ProcedureState};
use structopt::StructOpt;

use crate::{command::Command, context::Context, error::Error, procedure::Halt};

mod command;
mod context;
mod error;
mod procedure;

type Result<T> = StdResult<T, Error>;

fn run_procedure<P: Procedure>(context: &mut Context, mut proc: P) -> Result<()> {
    let mut context_dir = Context::get_context_dir()?;
    context_dir.push(Context::CONTEXT_FILE_NAME);

    let mut file = OpenOptions::new().write(true).open(context_dir)?;

    if !context.is_procedure_cached(P::NAME) {
        context.update_procedure_cache(
            P::NAME.to_string(),
            ProcedureCache::new(&P::State::INITIAL_STATE)?,
        );
        context.persist(&mut file)?;
    }

    let cache = &context[P::NAME];
    for proc_step in cache.cached_steps() {
        status!(("[CACHED] Skipping step {}: {}", proc_step.index, proc_step.description) => ());
    }

    let mut state = cache.current_state::<P::State>()?;

    loop {
        let cache = &mut context[P::NAME];
        if let Some(inner_state) = state {
            let step = cache.current_step();
            status!(("Step {}: {}", step.index, step.description) => {
                state = match proc.run(inner_state)? {
                    Halt::Yield(inner_state) => Some(inner_state),
                    Halt::Finish => None,
                };

                cache.advance(&state)?;
                context.persist(&mut file)?;
            });
        } else {
            break;
        };
    }

    Ok(())
}

fn main() {
    let wrapper = || -> Result<()> {
        let mut context = Context::load()?;

        match Command::from_args() {
            Command::Flash(proc) => run_procedure(&mut context, proc)?,
        }
        Ok(())
    };

    match wrapper() {
        Ok(_) => (),
        Err(error) => eprintln!("hoc error: {}", error),
    }
}
