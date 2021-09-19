use std::{fs::OpenOptions, result::Result as StdResult};

use context::ProcedureCache;
use hoclog::{error, status};
use procedure::{Procedure, ProcedureState};
use structopt::StructOpt;

use crate::{command::Command, context::Context, error::Error, procedure::Halt};

mod command;
mod context;
mod error;
mod procedure;

type Result<T> = StdResult<T, Error>;

fn run_procedure<P, S>(context: &mut Context, mut proc: P) -> Result<()>
where
    P: Procedure<State = S>,
    S: ProcedureState<Procedure = P>,
{
    let mut context_dir = Context::get_context_dir()?;
    context_dir.push(Context::CONTEXT_FILE_NAME);

    let mut file = OpenOptions::new().write(true).open(context_dir)?;

    if !context.is_procedure_cached(P::NAME) {
        context.update_procedure_cache(P::NAME.to_string(), ProcedureCache::new::<P::State>()?);
        context.persist(&mut file)?;
    }

    let mut invalidate_state = None;
    for step in context[P::NAME].completed_steps::<P::State>() {
        let (index, step) = step?;
        if step.needs_update(&proc) {
            invalidate_state.replace(index);
            break;
        }
        status!(
            ("Skipping step {}: {}", index, step.description()),
            label = "CACHED",
        );
    }

    if let Some(index) = invalidate_state {
        context[P::NAME].invalidate_state::<P::State>(index)?;
        context.persist(&mut file)?;
    }
    let mut state = context[P::NAME].current_state::<P::State>()?;

    loop {
        let cache = &mut context[P::NAME];
        if let Some(inner_state) = state {
            let index = cache.last_index();
            status!(("Step {}: {}", index, inner_state.description()) => {
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
        Ok(_) | Err(Error::LogError(_)) => (),
        Err(error) => {
            let _ = error!("hoc error: {}", error);
        }
    }
}
