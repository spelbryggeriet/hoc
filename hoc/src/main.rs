use std::result::Result as StdResult;

use context::ProcedureCache;
use hoclog::{error, info, status, warning};
use procedure::{Procedure, ProcedureState};
use structopt::StructOpt;

use crate::{
    command::Command,
    context::Context,
    error::Error,
    procedure::{Halt, ProcedureStateId},
};

mod command;
mod context;
mod error;
mod file_ref;
mod procedure;

type Result<T> = StdResult<T, Error>;

fn run_procedure<P, S>(context: &mut Context, mut proc: P) -> Result<()>
where
    P: Procedure<State = S>,
    S: ProcedureState<Procedure = P>,
{
    if !context.is_procedure_cached(P::NAME) {
        context.update_procedure_cache(P::NAME.to_string(), ProcedureCache::new::<P::State>()?);
        context.persist()?;
    }

    let mut invalidate_state = None;
    let cache = &context[P::NAME];
    'outer: for (index, step) in cache
        .completed_steps()
        .chain(cache.current_step())
        .enumerate()
    {
        if let Some(update_info) = step.state::<P::State>()?.needs_update(&proc)? {
            let state_id = step.id::<P::State>()?;
            if update_info.state_id > state_id {
                panic!(
                    "State with hash `{}` reported as invalid, but that state has not been run yet.",
                    update_info.state_id.to_hash(),
                );
            }

            for (rewind_index, rewind_step) in cache.completed_steps().enumerate() {
                if rewind_step.id::<P::State>()? == update_info.state_id {
                    if update_info.user_choice {
                        info!(
                            "Rewinding back to step {} ({}) because: {}.",
                            rewind_index + 1,
                            update_info.state_id.description(),
                            update_info.description,
                        );
                    } else {
                        warning!(
                            r#"Step {} ({}) is invalid because: {}. The script needs to rewind back to step {} ({})."#,
                            index + 1,
                            step.id::<P::State>()?.description(),
                            update_info.description,
                            rewind_index + 1,
                            update_info.state_id.description(),
                        )?;
                    }
                    invalidate_state.replace(update_info.state_id);
                    break 'outer;
                }
            }

            panic!(
                "Could not find state with hash `{}` in the completed steps.",
                update_info.state_id.to_hash(),
            );
        }
    }

    if let Some(state_id) = invalidate_state {
        context[P::NAME].invalidate_state::<P::State>(state_id)?;
        context.persist()?;
    }

    for (index, step) in context[P::NAME].completed_steps().enumerate() {
        status!(
            (
                "Skipping step {}: {}",
                index + 1,
                step.id::<P::State>()?.description(),
            ),
            label = "CACHED",
        );
    }

    let mut state = context[P::NAME].current_state::<P::State>()?;

    loop {
        let cache = &mut context[P::NAME];
        if let Some(inner_state) = state {
            let index = cache.completed_steps().count() + 1;
            status!(("Step {}: {}", index, inner_state.id().description()) => {
                state = match proc.run(inner_state)? {
                    Halt::Yield(inner_state) => Some(inner_state),
                    Halt::Finish => None,
                };

                cache.advance(&state)?;
                context.persist()?;
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
        Ok(_) | Err(Error::LogError(hoclog::Error::ErrorLogged)) => (),
        Err(error) => {
            let _ = error!("hoc error: {}", error);
        }
    }
}
