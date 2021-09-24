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
mod procedure;

type Result<T> = StdResult<T, Error>;

fn run_procedure<P, S>(context: &mut Context, mut proc: P) -> Result<()>
where
    P: Procedure<State = S>,
    S: ProcedureState<Procedure = P>,
{
    if !context.is_procedure_cached(P::NAME) {
        context.update_procedure_cache(P::NAME.to_string(), ProcedureCache::new::<S>()?);
        context.persist()?;
    }

    let mut invalidate_state = None;
    let cache = &context[P::NAME];
    'outer: for (index, step) in cache
        .completed_steps()
        .chain(cache.current_step())
        .enumerate()
    {
        if let Some(update_info) = step.state::<S>()?.needs_update(&proc)? {
            let state_id = step.id::<S>()?;
            if update_info.state_id > state_id {
                panic!(
                    "State with hash `{}` reported as invalid, but that state has not been run yet.",
                    update_info.state_id.to_hash(),
                );
            }

            let mut members: Vec<_> = S::Id::members()
                .filter(|m| *m <= update_info.state_id)
                .collect();
            members.sort();

            for (rewind_index, rewind_id) in members.into_iter().enumerate() {
                if rewind_id == update_info.state_id {
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
                            step.id::<S>()?.description(),
                            update_info.description,
                            rewind_index + 1,
                            update_info.state_id.description(),
                        )?;
                    }
                    invalidate_state.replace(update_info.state_id);
                    break 'outer;
                }
            }
        }
    }

    if let Some(state_id) = invalidate_state {
        context[P::NAME].invalidate_state::<S>(state_id)?;
        context.persist()?;
    }

    let mut index = 1;
    for (i, step) in context[P::NAME].completed_steps().enumerate() {
        status!(
            ("Skipping step {}: {}", i + 1, step.id::<S>()?.description(),),
            label = "CACHED",
        );
        index += 1;
    }

    loop {
        let cache = &mut context[P::NAME];
        if let Some(some_proc_step) = cache.current_step_mut() {
            let state_id = some_proc_step.id::<S>()?;
            status!(("Step {}: {}", index, state_id.description()) => {
                let state = match proc.run(some_proc_step)? {
                    Halt::Yield(inner_state) => Some(inner_state),
                    Halt::Finish => None,
                };

                cache.advance(&state)?;
                context.persist()?;
            });
            index += 1;
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
