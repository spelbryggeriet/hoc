use std::{
    process,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use colored::Colorize;
use lazy_static::lazy_static;
use structopt::StructOpt;

use hoclib::{
    Cache, Context, DirectoryState, FileStateDiff, HaltState, Procedure, ProcedureStateId,
    ProcedureStep,
};
use hoclog::{error, info, status, warning, LogErr};

use crate::command::Command;

mod command;

lazy_static! {
    static ref INTERRUPT: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
}

fn run_procedure<P: Procedure>(context: &mut Context, mut proc: P) -> hoclog::Result<()> {
    let proc_attributes = proc.get_attributes();

    info!("Procedure: {}", P::NAME);
    if !proc_attributes.is_empty() {
        info!("Attributes:");
        for (key, value) in proc_attributes.iter() {
            info!("  {}: {}", key, value);
        }
    }

    if !context.contains_cache(P::NAME, &proc_attributes) {
        context.update_cache(
            P::NAME.to_string(),
            proc_attributes.clone(),
            Cache::new::<P::State>().log_err()?,
        );
        context.persist().log_err()?;
    }

    let cache = &context[(P::NAME, &proc_attributes)];

    let mut invalidate_state = None;
    if let Some(state_id) = proc.rewind_state() {
        for (rewind_index, rewind_id) in cache
            .completed_steps()
            .map(ProcedureStep::id::<P::State>)
            .enumerate()
        {
            if rewind_id.log_err()? == state_id {
                invalidate_state.replace((rewind_index, state_id, None));
                break;
            }
        }
    }

    let work_dir = Context::get_work_dir().log_err()?;
    let mut valid_files = DirectoryState::new_unchecked(&work_dir);
    let mut cur_dir_state = DirectoryState::new_unchecked(work_dir);
    cur_dir_state.register_dir("").log_err()?;

    let completed_steps = cache.completed_steps().count();

    for (step, index) in cache
        .completed_steps()
        .rev()
        .zip((0..completed_steps).rev())
    {
        let mut step_dir_state = step.work_dir_state().clone();
        step_dir_state.unregister_files(&valid_files);
        let diff = step_dir_state.diff_files(&cur_dir_state);

        if !diff.is_empty() && invalidate_state.as_ref().map_or(true, |(j, ..)| index < *j) {
            invalidate_state.replace((index, step.id::<P::State>().log_err()?, Some(diff)));
            step_dir_state.unregister_files(&step_dir_state.changed_files(&cur_dir_state));
        }

        valid_files.merge(cur_dir_state.unregister_files(&step_dir_state));
    }

    if let Some((rewind_index, state_id, diff)) = invalidate_state {
        if let Some(diff) = diff {
            let diff_line = diff
                .iter()
                .map(FileStateDiff::to_string)
                .collect::<Vec<_>>()
                .join("\n");

            warning!(
                "Previously completed {} ({}) has become invalid because the working directory \
                     state has changed:\n\n{}",
                format!("Step {}", rewind_index + 1),
                state_id.description(),
                diff_line,
            )?;
        }

        info!(
            "Rewinding back to {} ({}).",
            format!("Step {}", rewind_index + 1).yellow(),
            state_id.description(),
        );

        context[(P::NAME, &proc_attributes)]
            .invalidate_state::<P::State>(state_id)
            .log_err()?;
        context.persist().log_err()?;
    }

    let mut index = 1;
    for (i, step) in context[(P::NAME, &proc_attributes)]
        .completed_steps()
        .enumerate()
    {
        hoclog::LOG
            .status(format!(
                "Skipping {}: {}",
                format!("Step {}", i + 1).yellow(),
                step.id::<P::State>().log_err()?.description()
            ))
            .with_label("CACHED".blue());
        index += 1;
    }

    loop {
        if INTERRUPT.load(Ordering::Relaxed) {
            error!("The program was interrupted.")?;
        }

        let cache = &mut context[(P::NAME, &proc_attributes)];
        if let Some(some_step) = cache.current_step_mut() {
            let state_id = some_step.id::<P::State>().log_err()?;
            status!("{}: {}", format!("Step {}", index).yellow(), state_id.description() => {
                let halt = proc.run(some_step)?;
                let state = match halt.state {
                    HaltState::Halt(inner_state) => Some(inner_state),
                    HaltState::Finish => None,
                };

                cache.advance(&state).log_err()?;
                if halt.persist {
                    context.persist().log_err()?;
                }
            });
            index += 1;
        } else {
            break;
        };
    }

    Ok(())
}

fn main() {
    let wrapper = || -> hoclog::Result<()> {
        ctrlc::set_handler(|| {
            if INTERRUPT.load(Ordering::Relaxed) {
                process::exit(1);
            } else {
                INTERRUPT.store(true, Ordering::Relaxed)
            }
        })
        .log_err()?;

        hoclib::reset_sudo_privileges().log_err()?;

        let mut context = Context::load().log_err()?;

        match Command::from_args() {
            Command::Flash(proc) => run_procedure(&mut context, proc)?,
            Command::Configure(proc) => run_procedure(&mut context, proc)?,
        }
        Ok(())
    };

    let _ = wrapper();
}
