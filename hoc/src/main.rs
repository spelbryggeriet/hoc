use std::result::Result as StdResult;

use hoclog::{error, info, status, warning};
use structopt::StructOpt;

use crate::{
    command::Command,
    context::{dir_state::DirectoryState, Context, ProcedureCache, ProcedureStep},
    error::Error,
    procedure::{Halt, Procedure, ProcedureStateId},
};

mod command;
mod context;
mod error;
mod procedure;

type Result<T> = StdResult<T, Error>;

fn run_procedure<P: Procedure>(context: &mut Context, mut proc: P) -> Result<()> {
    if !context.is_procedure_cached(P::NAME) {
        context.update_procedure_cache(P::NAME.to_string(), ProcedureCache::new::<P::State>()?);
        context.persist()?;
    }

    let mut invalidate_state = None;
    if let Some(state_id) = proc.rewind_state() {
        for (rewind_index, rewind_id) in context[P::NAME]
            .completed_steps()
            .map(ProcedureStep::id::<P::State>)
            .enumerate()
        {
            if rewind_id? == state_id {
                invalidate_state.replace((rewind_index, state_id));
                break;
            }
        }
    }

    let cur_dir_state = DirectoryState::get_snapshot(&Context::get_work_dir()?)?;
    for (index, step) in context[P::NAME]
        .completed_steps()
        .take(invalidate_state.as_ref().map_or(usize::MAX, |(i, _)| *i))
        .enumerate()
    {
        let diff = step.work_dir_state().diff(&cur_dir_state);
        if !diff.is_empty() {
            let state_id = step.id::<P::State>()?;

            warning!(
                "Previously completed step {} ({}) has become invalid because the working directory \
                 state has changed:\n\n{}",
                index + 1,
                state_id.description(),
                diff.changed_paths()
                    .into_iter()
                    .map(|(path, change_type)| format!(r#""{}" ({})"#, path, change_type))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )?;

            invalidate_state.replace((index, state_id));
            break;
        }
    }

    if let Some((rewind_index, state_id)) = invalidate_state {
        info!(
            "Rewinding back to step {} ({}).",
            rewind_index + 1,
            state_id.description(),
        );

        context[P::NAME].invalidate_state::<P::State>(state_id)?;
        context.persist()?;
    }

    let mut index = 1;
    for (i, step) in context[P::NAME].completed_steps().enumerate() {
        status!(
            (
                "Skipping step {}: {}",
                i + 1,
                step.id::<P::State>()?.description(),
            ),
            label = "CACHED",
        );
        index += 1;
    }

    loop {
        let cache = &mut context[P::NAME];
        if let Some(some_proc_step) = cache.current_step_mut() {
            let state_id = some_proc_step.id::<P::State>()?;
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
