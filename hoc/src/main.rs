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

use hoc_core::{
    history,
    procedure::{self, Id, Procedure, State},
    Context,
};
use hoc_log::{bail, error, info, status, warning, LogErr};

use crate::command::Command;

mod command;

const ERR_MSG_PARSE_ID: &str = "Parsing procedure step ID";
const ERR_MSG_PERSIST_CONTEXT: &str = "Persisting context";

lazy_static! {
    static ref INTERRUPT: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
}

#[derive(StructOpt)]
struct MainCommand {
    #[structopt(flatten)]
    procedure: Command,

    #[structopt(long)]
    rerun: bool,
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

fn index_to_string(index: &hoc_core::history::Index) -> String {
    if let Some(attr_list) =
        list_string(index.attributes().iter(), |(k, v)| format!("{}: {}", k, v))
    {
        format!("{}\n{attr_list}", index.name())
    } else {
        index.name().to_string()
    }
}

fn validate_registry_state(context: &mut Context) -> hoc_log::Result<()> {
    let changes = context.registry().validate()?;
    let changes_list = list_string(changes.iter(), |(_, c)| format!("{}", c));

    if let Some(changes_list) = changes_list {
        info!("The registry state has changed:\n{}", changes_list);

        let change_keys: Vec<_> = changes.into_iter().map(|(k, _)| k).collect();
        let affected_procedures: Vec<_> = context
            .history()
            .iter()
            .filter_map(|(index, item)| {
                item.registry_keys()
                    .iter()
                    .any(|k| change_keys.contains(k))
                    .then(|| index.clone())
            })
            .collect();

        let procedures_list = list_string(affected_procedures.iter(), index_to_string);
        if let Some(procedures_list) = procedures_list {
            warning!(
                "The following procedure histories will be reset:\n{}",
                procedures_list,
            )
            .get()?;

            let history = context.history_mut();
            let invalid_keys: Vec<_> = affected_procedures
                .iter()
                .flat_map(|i| history.item(&i).registry_keys())
                .cloned()
                .collect();

            for index in affected_procedures {
                history.remove_item(&index)?;
            }

            let registry = context.registry_mut();
            for key in invalid_keys {
                registry.remove(key)?;
            }

            context.persist().log_context(ERR_MSG_PERSIST_CONTEXT)?;
        }
    }

    Ok(())
}

fn check_dependencies<P: Procedure>(context: &Context) -> hoc_log::Result<()> {
    let history = context.history();
    let mut incomplete_deps: Vec<_> = history
        .iter()
        .filter_map(|(index, item)| {
            (P::DEPENDENCIES.contains(&index.name()) && !item.is_complete()).then(|| index.name())
        })
        .chain(
            P::DEPENDENCIES
                .iter()
                .copied()
                .filter(|d| history.indices().all(|i| i.name() != *d)),
        )
        .collect();
    incomplete_deps.sort();
    let incomplete_deps_list = list_string(incomplete_deps.iter(), <&str>::to_string);
    if let Some(incomplete_deps) = incomplete_deps_list {
        bail!(
            "The following procedures are required to be run before '{}':\n{incomplete_deps}",
            P::NAME
        );
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
    let dependencies = P::DEPENDENCIES;
    let history = context.history();

    let affected_procedures: Vec<_> = history
        .indices()
        .filter_map(|index| {
            (index != history_index && dependencies.contains(&index.name())).then(|| index.clone())
        })
        .collect();
    let procedures_list = list_string(affected_procedures.iter(), index_to_string);

    let history = context.history_mut();
    if let Some(procedures_list) = procedures_list {
        let proc_name = P::NAME;
        warning!("The following procedures depend on '{proc_name}' and need to be reset:\n{procedures_list}").get()?;
        for index in affected_procedures {
            history.remove_item(&index)?;
        }
    }

    history.remove_item(history_index)?;
    history.add_item(proc)?;

    let keys_to_remove: Vec<_> = context
        .registry()
        .get_keys()
        .into_iter()
        .filter(|k| {
            context
                .history()
                .items()
                .all(|item| item.registry_keys().iter().all(|rk| k != rk))
        })
        .collect();

    let registry = context.registry_mut();
    for key in keys_to_remove {
        registry.remove(key)?;
    }

    context.persist().log_context(ERR_MSG_PERSIST_CONTEXT)?;

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

fn run_procedure<P: Procedure>(
    context: &mut Context,
    mut proc: P,
    rerun: bool,
) -> hoc_log::Result<()> {
    let history_index = status!("Pre-check").on(|| {
        let history_index = get_history_index(context, &proc)?;

        status!("Validate registry state").on(|| validate_registry_state(context))?;
        status!("Check dependencies").on(|| check_dependencies::<P>(context))?;
        if rerun {
            let state_id = P::State::default().id();
            status!(
                "Rewind back to {} ({})",
                "Step 1".yellow(),
                state_id.description(),
            )
            .on(|| rewind_history(context, &proc, &history_index))?;
        }

        hoc_log::Result::Ok(history_index)
    })?;

    let proc_name = history_index.name().blue();
    status!("Run procedure {proc_name}").on(|| {
        let attrs_list = list_string(history_index.attributes().iter(), |(k, v)| {
            format!("{}: {}", k, v)
        });
        if let Some(attrs_list) = attrs_list {
            info!("Attributes:\n{}", attrs_list);
        }

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

        let main_command = MainCommand::from_args();
        let rerun = main_command.rerun;
        match main_command.procedure {
            Command::CreateUser(proc) => run_procedure(&mut context, proc, rerun)?,
            Command::DownloadImage(proc) => run_procedure(&mut context, proc, rerun)?,
            Command::PrepareSdCard(proc) => run_procedure(&mut context, proc, rerun)?,
            Command::Init(proc) => run_procedure(&mut context, proc, rerun)?,
        }
        Ok(())
    };

    let _ = wrapper();
}
