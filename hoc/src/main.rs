use std::{
    path::PathBuf,
    process,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use colored::Colorize;
use lazy_static::lazy_static;
use structopt::StructOpt;

use hoc_core::{
    history,
    kv::WriteStore,
    procedure::{self, Id, Procedure},
    Context,
};
use log::{error, info, status, warning, LogErr};

use crate::command::Command;

mod command;

const ERR_MSG_PARSE_ID: &str = "Parsing procedure step ID";
const ERR_MSG_PERSIST_CONTEXT: &str = "Persisting context";

lazy_static! {
    static ref INTERRUPT: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
}

fn list_string<T>(list: &[T], to_string: impl Fn(&T) -> String) -> Option<String> {
    let padded_to_string = |e, c| {
        let s = to_string(e);
        if let Some((head, tail)) = s.split_once('\n') {
            head.to_string()
                + &tail
                    .lines()
                    .map(|l| format!("\n{c} {l}"))
                    .collect::<String>()
        } else {
            s
        }
    };

    match list {
        [init @ .., last] => {
            let init_string = init
                .into_iter()
                .map(|e| format!("├╴{}\n", padded_to_string(e, '│')))
                .collect::<String>();

            Some(format!("{init_string}└╴{}", padded_to_string(last, ' ')))
        }
        _ => None,
    }
}

fn validate_registry_state(context: &mut Context) -> log::Result<()> {
    let changes = context.registry().validate()?;
    let changes_list = list_string(changes.as_slice(), |(_, c)| format!("{}", c));

    if let Some(changes_list) = changes_list {
        info!("The registry state has changed:\n{}", changes_list);

        let change_keys: Vec<_> = changes.into_iter().map(|(k, _)| k).collect();
        let history_keys: Vec<_> = context.history().indices().collect();
        let mut affected_procedures: Vec<_> = change_keys
            .iter()
            .filter_map(|key| {
                history_keys
                    .iter()
                    .find(|index| key.starts_with(PathBuf::from(**index)))
            })
            .collect();
        affected_procedures.sort();
        affected_procedures.dedup();

        let procedures_list = list_string(affected_procedures.as_slice(), |i| {
            if let Some(attr_list) =
                list_string(i.attributes(), |a| format!("{}: {}", a.key, a.value))
            {
                format!("{}\n{attr_list}", i.name())
            } else {
                i.name().to_string()
            }
        });

        if let Some(procedures_list) = procedures_list {
            warning!(
                "The following procedure histories will be reset:\n{}",
                procedures_list
            )?;

            let indices: Vec<_> = affected_procedures.into_iter().copied().cloned().collect();
            let history = context.history_mut();

            for index in indices {
                history.remove_item(&index)?;
            }

            let registry = context.registry_mut();
            for key in change_keys {
                registry.remove(key)?;
            }

            context.persist()?;
        }
    }

    Ok(())
}

fn get_history_index<P: Procedure>(context: &mut Context, proc: &P) -> log::Result<history::Index> {
    if let Some(history_index) = context.history().get_index(proc) {
        Ok(history_index)
    } else {
        let history_index = context.history_mut().add_item(proc)?;
        context.persist().log_context(ERR_MSG_PERSIST_CONTEXT)?;
        Ok(history_index)
    }
}

fn rewind_history<P: Procedure>(
    context: &mut Context,
    proc: &P,
    history_index: &history::Index,
) -> log::Result<()> {
    let history_item = context.history_mut().item_mut(history_index);
    if let Some(state_id) = proc.rewind_state() {
        let mut iter = history_item
            .completed()
            .iter()
            .map(procedure::Step::id::<P::State>)
            .enumerate();

        while let Some((rewind_index, rewind_id)) = iter.next() {
            if rewind_id.log_context(ERR_MSG_PARSE_ID)? == state_id {
                info!(
                    "Rewinding back to {} ({}).",
                    format!("Step {}", rewind_index + 1).yellow(),
                    state_id.description(),
                );

                drop(iter);
                history_item
                    .invalidate::<P::State>(state_id)
                    .log_context("Invalidating history item")?;
                context.persist().log_context(ERR_MSG_PERSIST_CONTEXT)?;
                break;
            }
        }
    }

    Ok(())
}

fn get_step_index<P: Procedure>(
    context: &Context,
    history_index: &history::Index,
) -> log::Result<usize> {
    let steps = context.history().item(history_index);
    let mut step_index = 1;
    for (i, step) in steps.completed().iter().enumerate() {
        log::LOG
            .status(format!(
                "Skipping {}: {}",
                format!("Step {}", i + 1).yellow(),
                &step
                    .id::<P::State>()
                    .log_context(ERR_MSG_PARSE_ID)?
                    .description(),
            ))
            .with_label("completed".blue());
        step_index += 1;
    }

    Ok(step_index)
}

fn run_step<P: Procedure>(
    context: &mut Context,
    proc: &mut P,
    history_index: &history::Index,
    state: P::State,
) -> log::Result<()> {
    let global_registry = context.registry_mut();
    let proc_registry = global_registry.split(history_index)?;

    let halt = proc.run(state, &proc_registry, global_registry)?;
    let state = match halt.state {
        procedure::HaltState::Halt(inner_state) => Some(inner_state),
        procedure::HaltState::Finish => None,
    };

    global_registry.merge(proc_registry)?;
    context
        .history_mut()
        .item_mut(&history_index)
        .next(&state)?;

    if halt.persist {
        {
            status!("Persist context");
            context.persist().log_context(ERR_MSG_PERSIST_CONTEXT)?;
        }
    }

    Ok(())
}

fn run_loop<P: Procedure>(
    context: &mut Context,
    proc: &mut P,
    history_index: &history::Index,
) -> log::Result<()> {
    let mut step_index = get_step_index::<P>(context, history_index)?;

    loop {
        if INTERRUPT.load(Ordering::Relaxed) {
            error!("The program was interrupted.")?;
        }

        if let Some(step) = context.history().item(history_index).current() {
            let state = step.state::<P::State>()?;
            let state_id = step.id::<P::State>().log_context(ERR_MSG_PARSE_ID)?;

            {
                status!(
                    "{}: {}",
                    format!("Step {step_index}").yellow(),
                    state_id.description()
                );
                run_step(context, proc, &history_index, state)?;
            }

            step_index += 1;
        } else {
            break Ok(());
        };
    }
}

fn run_procedure<P: Procedure>(context: &mut Context, mut proc: P) -> log::Result<()> {
    {
        status!("Validate registry state");
        validate_registry_state(context)?;
    }

    let history_index = get_history_index(context, &proc)?;
    {
        status!("Run procedure {}", history_index.name().blue());

        if let Some(attrs_list) = list_string(history_index.attributes(), |a| {
            format!("{}: {}", a.key, a.value)
        }) {
            info!("Attributes:\n{}", attrs_list);
        }

        rewind_history(context, &proc, &history_index)?;
        run_loop(context, &mut proc, &history_index)?;
    };

    Ok(())
}

fn main() {
    let wrapper = || -> log::Result<()> {
        ctrlc::set_handler(|| {
            if INTERRUPT.load(Ordering::Relaxed) {
                process::exit(1);
            } else {
                INTERRUPT.store(true, Ordering::Relaxed)
            }
        })
        .log_context("Setting interrupt handler")?;

        hoc_core::reset_sudo_privileges().log_context("Resetting sudo privileges")?;

        let mut context = Context::load().log_context("Loading context")?;

        match Command::from_args() {
            Command::CreateUser(proc) => run_procedure(&mut context, proc)?,
            Command::DownloadImage(proc) => run_procedure(&mut context, proc)?,
            Command::PrepareSdCard(proc) => run_procedure(&mut context, proc)?,
            Command::Init(proc) => run_procedure(&mut context, proc)?,
        }
        Ok(())
    };

    let _ = wrapper();
}
