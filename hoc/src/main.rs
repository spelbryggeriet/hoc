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

use hoclib::{Context, HaltState, Procedure, ProcedureStateId, ProcedureStep};
use hoclog::{error, info, status, warning, LogErr};

use crate::command::Command;

mod command;

const ERR_MSG_PARSE_ID: &str = "Parsing procedure step ID";
const ERR_MSG_PERSIST_CONTEXT: &str = "Persisting context";

lazy_static! {
    static ref INTERRUPT: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
}

fn run_procedure<P: Procedure>(context: &mut Context, mut proc: P) -> hoclog::Result<()> {
    let steps_index = if let Some(steps_index) = context.get_steps_index(&proc) {
        steps_index
    } else {
        let steps_index = context.add_steps::<P>(&proc).log_context("Adding steps")?;
        context.persist().log_context(ERR_MSG_PERSIST_CONTEXT)?;
        steps_index
    };

    info!("Procedure: {}", steps_index.name());
    let attrs = steps_index.attributes();
    if !attrs.is_empty() {
        let mut proc_info = "Attributes:".to_string();
        for (i, (key, value)) in attrs.iter().enumerate() {
            if i < attrs.len() - 1 {
                proc_info += &format!("\n├╴{}: {}", key, value);
            } else {
                proc_info += &format!("\n└╴{}: {}", key, value);
            }
        }
        info!(proc_info);
    }

    if let Some(state_id) = proc.rewind_state() {
        let mut iter = context[&steps_index]
            .completed()
            .iter()
            .map(ProcedureStep::id::<P::State>)
            .enumerate();

        while let Some((rewind_index, rewind_id)) = iter.next() {
            if rewind_id.log_context(ERR_MSG_PARSE_ID)? == state_id {
                info!(
                    "Rewinding back to {} ({}).",
                    format!("Step {}", rewind_index + 1).yellow(),
                    state_id.description(),
                );

                drop(iter);
                context[&steps_index]
                    .invalidate_state::<P::State>(state_id)
                    .log_context("Invalidating procedure step state")?;
                context.persist().log_context(ERR_MSG_PERSIST_CONTEXT)?;
                break;
            }
        }
    }

    let steps = &context[&steps_index];
    if let arr @ &[_, ..] = steps.added_paths(&proc)?.as_slice() {
        warning!(
            "Untracked paths have been added to the working directory and will be removed:\n{}",
            arr.into_iter()
                .map(|p| format!("  - {}", p.to_string_lossy()))
                .collect::<Vec<_>>()
                .join("\n")
        )?;
    }

    if let Some((step_index, state_id)) = steps
        .oldest_invalid_state(&proc)
        .log_context("Retrieving oldest invalid state")?
    {
        warning!(
            "Previously completed Step {} ({}) has become invalid because the working directory \
            state has changed.",
            step_index + 1,
            state_id.description(),
        )?;

        info!(
            "Rewinding back to {} ({}).",
            format!("Step {}", step_index + 1).yellow(),
            state_id.description(),
        );

        context[&steps_index]
            .invalidate_state::<P::State>(state_id)
            .log_context("Invalidating procedure step state")?;
        context.persist().log_context(ERR_MSG_PERSIST_CONTEXT)?;
    }

    let mut index = 1;
    for (i, step) in context[&steps_index].completed().iter().enumerate() {
        hoclog::LOG
            .status(format!(
                "Skipping {}: {}",
                format!("Step {}", i + 1).yellow(),
                step.id::<P::State>()
                    .log_context(ERR_MSG_PARSE_ID)?
                    .description()
            ))
            .with_label("completed".blue());
        index += 1;
    }

    loop {
        if INTERRUPT.load(Ordering::Relaxed) {
            error!("The program was interrupted.")?;
        }

        let steps = &mut context[&steps_index];
        if let Some(some_step) = steps.current_mut() {
            let state_id = some_step.id::<P::State>().log_context(ERR_MSG_PARSE_ID)?;
            status!("{}: {}", format!("Step {}", index).yellow(), state_id.description() => {
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
        .log_context("Setting interrupt handler")?;

        hoclib::reset_sudo_privileges().log_context("Resetting sudo privileges")?;

        let mut context = Context::load().log_context("Loading context")?;

        match Command::from_args() {
            Command::Flash(proc) => run_procedure(&mut context, proc)?,
            Command::Configure(proc) => run_procedure(&mut context, proc)?,
        }
        Ok(())
    };

    let _ = wrapper();
}
