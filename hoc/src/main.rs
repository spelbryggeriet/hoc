use std::result::Result as StdResult;

use context::dir_state::FileStateDiff;
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

    let cache = &context[P::NAME];

    let mut invalidate_state = None;
    if let Some(state_id) = proc.rewind_state() {
        for (rewind_index, rewind_id) in cache
            .completed_steps()
            .map(ProcedureStep::id::<P::State>)
            .enumerate()
        {
            if rewind_id? == state_id {
                invalidate_state.replace((rewind_index, state_id, None));
                break;
            }
        }
    }

    let mut cur_dir_state = DirectoryState::get_snapshot(&Context::get_work_dir()?)?;
    let mut valid_files = DirectoryState::new(Context::WORK_DIR);
    let completed_steps = cache.completed_steps().count();
    for (step, index) in cache
        .completed_steps()
        .rev()
        .zip((0..completed_steps).rev())
    {
        let mut step_dir_state = step.work_dir_state().clone();
        step_dir_state.remove_files(&valid_files);
        let diff = step_dir_state.file_changes(&cur_dir_state);

        if !diff.is_empty() && invalidate_state.as_ref().map_or(true, |(j, ..)| index < *j) {
            invalidate_state.replace((index, step.id::<P::State>()?, Some(diff)));
            step_dir_state.remove_files(&step_dir_state.changed_files(&cur_dir_state));
        }

        valid_files.merge(cur_dir_state.remove_files(&step_dir_state));
    }

    if let Some((rewind_index, state_id, diff)) = invalidate_state {
        if let Some(diff) = diff {
            let diff_line = diff
                .iter()
                .map(FileStateDiff::to_string)
                .collect::<Vec<_>>()
                .join("\n");

            warning!(
                "Previously completed step {} ({}) has become invalid because the working directory \
                     state has changed:\n\n{}",
                rewind_index + 1,
                state_id.description(),
                diff_line,
            )?;
        }

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
        hoclog::LOG
            .status(format!(
                "Skipping step {}: {}",
                i + 1,
                step.id::<P::State>()?.description()
            ))
            .with_label("CACHED");
        index += 1;
    }

    loop {
        let cache = &mut context[P::NAME];
        if let Some(some_proc_step) = cache.current_step_mut() {
            let state_id = some_proc_step.id::<P::State>()?;
            status!(("Step {}: {}", index, state_id.description()), {
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
