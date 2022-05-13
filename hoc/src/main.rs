use std::{
    collections::HashSet,
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
    procedure::{self, Id, Procedure},
    Context,
};
use hoc_log::{error, info, status, warning, LogErr};

use crate::command::Command;

mod command;

const ERR_MSG_PARSE_ID: &str = "Parsing procedure step ID";
const ERR_MSG_PERSIST_CONTEXT: &str = "Persisting context";

lazy_static! {
    static ref INTERRUPT: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
}

fn list_string<T>(
    mut iter: impl Iterator<Item = T>,
    to_string: impl Fn(T) -> String,
) -> Option<String> {
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

    let mut current = iter.next();
    let mut result = String::new();
    while let next @ Some(_) = iter.next() {
        result += "├╴";
        result += &padded_to_string(current.unwrap(), '|');
        result += "\n";
        current = next;
    }

    if let Some(last) = current {
        result += "└╴";
        result += &padded_to_string(last, ' ');
        Some(result)
    } else {
        None
    }
}

fn validate_registry_state(context: &mut Context) -> hoc_log::Result<()> {
    let changes = context.registry().validate()?;
    let changes_list = list_string(changes.iter(), |(_, c)| format!("{}", c));

    if let Some(changes_list) = changes_list {
        info!("The registry state has changed:\n{}", changes_list);

        let change_keys: Vec<_> = changes.into_iter().map(|(k, _)| k).collect();
        let history_iter: Vec<_> = context.history().iter().collect();
        let affected_procedures: HashSet<_> = change_keys
            .iter()
            .flat_map(|key| {
                history_iter.iter().filter_map(move |(index, item)| {
                    item.registry_keys()
                        .iter()
                        .any(|k| k == key)
                        .then(|| *index)
                })
            })
            .collect();

        let procedures_list = list_string(
            affected_procedures.iter().copied(),
            |i: &hoc_core::history::Index| {
                if let Some(attr_list) =
                    list_string(i.attributes().iter(), |(k, v)| format!("{}: {}", k, v))
                {
                    format!("{}\n{attr_list}", i.name())
                } else {
                    i.name().to_string()
                }
            },
        );
        if let Some(procedures_list) = procedures_list {
            warning!(
                "The following procedure histories will be reset:\n{}",
                procedures_list,
            )
            .get()?;

            let indices: Vec<_> = affected_procedures.into_iter().cloned().collect();
            let history = context.history_mut();
            let invalid_keys: Vec<_> = indices
                .iter()
                .flat_map(|i| history.item(&i).registry_keys())
                .cloned()
                .collect();

            for index in indices {
                history.remove_item(&index)?;
            }

            let registry = context.registry_mut();
            for key in invalid_keys {
                registry.remove(key)?;
            }

            context.persist()?;
        }
    }

    Ok(())
}

fn get_history_index<P: Procedure>(
    context: &mut Context,
    proc: &P,
) -> hoc_log::Result<history::Index> {
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
) -> hoc_log::Result<()> {
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
) -> hoc_log::Result<usize> {
    let steps = context.history().item(history_index);
    let mut index = 1;
    for step in steps.completed().iter() {
        let step_str = format!("Step {index}").yellow();
        let step_id_str = step
            .id::<P::State>()
            .log_context(ERR_MSG_PARSE_ID)?
            .description();

        status!("Skipping {step_str}: {step_id_str}")
            .with_label("completed".blue())
            .finish();

        index += 1;
    }

    Ok(index)
}

fn run_step<P: Procedure>(
    context: &mut Context,
    proc: &mut P,
    history_index: &history::Index,
    state: P::State,
) -> hoc_log::Result<()> {
    let registry = context.registry_mut();
    let record = registry.record_accesses();

    let halt = proc.run(state, registry)?;
    let state = match halt.state {
        procedure::HaltState::Halt(inner_state) => Some(inner_state),
        procedure::HaltState::Finish => None,
    };

    context
        .history_mut()
        .item_mut(&history_index)
        .next(&state, record.finish())?;

    if halt.persist {
        status!("Persist context").on(|| context.persist().log_context(ERR_MSG_PERSIST_CONTEXT))?;
    }

    Ok(())
}

fn run_loop<P: Procedure>(
    context: &mut Context,
    proc: &mut P,
    history_index: &history::Index,
) -> hoc_log::Result<()> {
    let mut step_index = get_step_index::<P>(context, history_index)?;

    loop {
        if INTERRUPT.load(Ordering::Relaxed) {
            error!("The program was interrupted.")?;
        }

        if let Some(step) = context.history().item(history_index).current() {
            let state = step.state::<P::State>()?;
            let state_id = step.id::<P::State>().log_context(ERR_MSG_PARSE_ID)?;
            let step_str = format!("Step {step_index}").yellow();
            let state_str = state_id.description();

            status!("{step_str}: {state_str}")
                .on(|| run_step(context, proc, &history_index, state))?;

            step_index += 1;
        } else {
            break Ok(());
        };
    }
}

fn run_procedure<P: Procedure>(context: &mut Context, mut proc: P) -> hoc_log::Result<()> {
    status!("Validate registry state").on(|| validate_registry_state(context))?;

    let history_index = get_history_index(context, &proc)?;
    let proc_name = history_index.name().blue();
    status!("Run procedure {proc_name}").on(|| {
        let attrs_list = list_string(history_index.attributes().iter(), |(k, v)| {
            format!("{}: {}", k, v)
        });
        if let Some(attrs_list) = attrs_list {
            info!("Attributes:\n{}", attrs_list);
        }

        rewind_history(context, &proc, &history_index)?;
        run_loop(context, &mut proc, &history_index)
    })
}

fn main() {
    let wrapper = || -> hoc_log::Result<()> {
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
