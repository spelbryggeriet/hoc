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

use hoclib::{
    history,
    kv::WriteStore,
    procedure::{Attribute, HaltState, Id, Procedure, Step},
    Context,
};
use hoclog::{error, info, status, warning, LogErr};

use crate::command::Command;

mod command;

const ERR_MSG_PARSE_ID: &str = "Parsing procedure step ID";
const ERR_MSG_PERSIST_CONTEXT: &str = "Persisting context";

lazy_static! {
    static ref INTERRUPT: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
}

fn index_to_key(index: &history::Index) -> PathBuf {
    format!(
        "{}{}",
        index.name(),
        index
            .attributes()
            .iter()
            .map(|attr| format!("/{}", attr.value))
            .collect::<String>()
    )
    .into()
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

fn run_procedure<P: Procedure>(context: &mut Context, mut proc: P) -> hoclog::Result<()> {
    status!("Validate registry state" => {
        let changes = context.registry().validate()?;
        let changes_list = list_string(changes.as_slice(), |(_, c)| format!("{}", c));

        if let Some(changes_list) = changes_list {
            info!("The registry state has changed:\n{}", changes_list);

            let change_keys: Vec<_> = changes.into_iter().map(|(k, _)| k).collect();
            let history_keys: Vec<_> = context.history().indices().collect();
            let mut affected_procedures: Vec<_> = change_keys
                .iter()
                .filter_map(|key| {
                    history_keys.iter().copied().find(|index| {
                        key.starts_with(index_to_key(index))
                    })
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

                let indices: Vec<_> = affected_procedures
                    .into_iter()
                    .cloned()
                    .collect();
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
    });

    let history_index = if let Some(history_index) = context.history().get_index(&proc) {
        history_index
    } else {
        let history_index = context.history_mut().add_item(&proc)?;
        context.persist().log_context(ERR_MSG_PERSIST_CONTEXT)?;
        history_index
    };

    status!("Run procedure {}", history_index.name().blue() => {
        let attrs = history_index.attributes();
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

        rewind_dir_state(context, &proc, &history_index)?;

        let steps = context.history().item(&history_index);
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

            if let Some(step) = context.history().item(&history_index).current() {
                let state_id = step.id::<P::State>().log_context(ERR_MSG_PARSE_ID)?;

                status!("{}: {}", format!("Step {}", index).yellow(), state_id.description() => {
                    let state = step.state::<P::State>()?;
                    let global_registry = context.registry_mut();
                    let proc_registry = global_registry.split(index_to_key(&history_index))?;

                    let halt = proc.run(state, &proc_registry, global_registry)?;
                    let state = match halt.state {
                        HaltState::Halt(inner_state) => Some(inner_state),
                        HaltState::Finish => None,
                    };

                    global_registry.merge(proc_registry)?;
                    context.history_mut().item_mut(&history_index).next(&state)?;

                    if halt.persist {
                        status!("Persist context" => {
                            context.persist().log_context(ERR_MSG_PERSIST_CONTEXT)?;
                        });
                    }

                });

                index += 1;
            } else {
                break;
            };
        }
    });

    Ok(())
}

fn rewind_dir_state<P: Procedure>(
    context: &mut Context,
    proc: &P,
    history_index: &history::Index,
) -> hoclog::Result<()> {
    let history_item = context.history_mut().item_mut(&history_index);
    if let Some(state_id) = proc.rewind_state() {
        let mut iter = history_item
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
            Command::PrepareSdCard(proc) => run_procedure(&mut context, proc)?,
            Command::Init(proc) => run_procedure(&mut context, proc)?,
        }
        Ok(())
    };

    let _ = wrapper();
}
