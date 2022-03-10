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
    procedure::{Attribute, HaltState, Id, Procedure, Step},
    Context, StepHistoryIndex,
};
use hoclog::{error, info, status, warning, LogErr};

use crate::command::Command;

mod command;

const ERR_MSG_PARSE_ID: &str = "Parsing procedure step ID";
const ERR_MSG_PERSIST_CONTEXT: &str = "Persisting context";

lazy_static! {
    static ref INTERRUPT: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
}

fn run_procedure<P: Procedure>(context: &mut Context, mut proc: P) -> hoclog::Result<()> {
    let step_history_index = if let Some(step_history_index) = context.get_step_history_index(&proc)
    {
        step_history_index
    } else {
        let step_history_index = context
            .add_step_history::<P>(&proc)
            .log_context("Adding steps")?;
        context.persist().log_context(ERR_MSG_PERSIST_CONTEXT)?;
        step_history_index
    };

    info!("Procedure: {}", step_history_index.name());
    let attrs = step_history_index.attributes();
    if !attrs.is_empty() {
        let mut proc_info = "Attributes:".to_string();
        for (i, Attribute { key, value }) in attrs.iter().enumerate() {
            if i < attrs.len() - 1 {
                proc_info += &format!("\n├╴{key}: {value}");
            } else {
                proc_info += &format!("\n└╴{key}: {value}");
            }
        }
        info!(proc_info);
    }

    rewind_dir_state(context, &proc, &step_history_index)?;

    let steps = &context[&step_history_index];
    let mut index = 1;
    for (i, step) in steps.completed().iter().enumerate() {
        hoclog::LOG
            .status(format!(
                "Skipping {}: {}",
                format!("Step {}", i + 1).yellow(),
                &step
                    .id::<P::State>()
                    .log_context(ERR_MSG_PARSE_ID)?
                    .description(),
            ))
            .with_label("completed".blue());
        index += 1;
    }

    loop {
        if INTERRUPT.load(Ordering::Relaxed) {
            error!("The program was interrupted.")?;
        }

        let steps = &mut context[&step_history_index];
        if let Some(some_step) = steps.current_mut() {
            let state_id = some_step.id::<P::State>().log_context(ERR_MSG_PARSE_ID)?;

            status!(
                "{}: {}",
                format!("Step {}", index).yellow(),
                state_id.description() => {
                    let halt = proc.run(some_step)?;
                    let state = match halt.state {
                        HaltState::Halt(inner_state) => Some(inner_state),
                        HaltState::Finish => None,
                    };

                    if state.is_some() {
                        status!("Save directory snapshot" => {
                            steps.next(&state)?;
                        });
                    } else {
                        steps.next(&state)?;
                    }

                    if halt.persist {
                        context.persist().log_context(ERR_MSG_PERSIST_CONTEXT)?;
                    }
                }
            );
            index += 1;
        } else {
            break;
        };
    }

    Ok(())
}

fn rewind_dir_state<P: Procedure>(
    context: &mut Context,
    proc: &P,
    step_history_index: &StepHistoryIndex,
) -> hoclog::Result<()> {
    let step_history = &mut context[&step_history_index];
    let cur_dir_state = status!("Check directory state" => {
        let cur_dir_state = step_history.read_work_dir_state()?;

        if step_history.is_work_dir_corrupt(&cur_dir_state) {
            warning!(
                "The working directory state has changed and will be restored to the last valid state.",
            )?;
        } else if let [ref heads @ .., last] = step_history
            .added_work_dir_paths(&cur_dir_state)?
            .as_slice()
        {
            warning!(
                "Untracked paths have been added to the working directory and will be removed:{}\n└╴{}",
                heads
                    .into_iter()
                    .map(|p| format!("\n├╴{}", p.to_string_lossy()))
                    .collect::<Vec<_>>()
                    .join(""),
                last.to_string_lossy(),
            )?;
        }

        cur_dir_state
    });

    if let Some(state_id) = proc.rewind_state() {
        let mut iter = step_history
            .completed()
            .iter()
            .map(Step::id::<P::State>)
            .enumerate();

        while let Some((rewind_index, rewind_id)) = iter.next() {
            if rewind_id.log_context(ERR_MSG_PARSE_ID)? == state_id {
                info!(
                    "Rewinding back to {} ({}).",
                    format!("Step {}", rewind_index + 1).yellow(),
                    state_id.description(),
                );

                drop(iter);
                step_history
                    .invalidate_state::<P::State>(state_id)
                    .log_context("Invalidating procedure step state")?;
                context.persist().log_context(ERR_MSG_PERSIST_CONTEXT)?;
                break;
            }
        }
    }

    let step_history = &mut context[&step_history_index];
    if let Some((step_index, state_id)) = step_history
        .restore_work_dir::<P::State>(cur_dir_state)
        .log_context("Restoring working directory")?
    {
        info!(
            "Rewinding back to {} ({}).",
            format!("Step {}", step_index + 1).yellow(),
            state_id.description(),
        );

        step_history
            .invalidate_state::<P::State>(state_id)
            .log_context("Invalidating procedure step state")?;
        context.persist().log_context(ERR_MSG_PERSIST_CONTEXT)?;
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
        .log_context("Setting interrupt handler")?;

        hoclib::reset_sudo_privileges().log_context("Resetting sudo privileges")?;

        let mut context = Context::load().log_context("Loading context")?;

        match Command::from_args() {
            Command::CreateUser(proc) => run_procedure(&mut context, proc)?,
            Command::DownloadImage(proc) => run_procedure(&mut context, proc)?,
            Command::Flash(proc) => run_procedure(&mut context, proc)?,
            Command::Configure(proc) => run_procedure(&mut context, proc)?,
        }
        Ok(())
    };

    let _ = wrapper();
}
