#[macro_use]
extern crate strum_macros;

#[macro_use]
mod log;

mod context;
mod file;
mod service;

mod build;
mod configure;
mod deploy;
mod flash;
mod publish;

mod prelude {
    pub use crate::file::{NamedFile, TempDir};
    pub use crate::log::{Styling, Wrapping};
    pub use crate::LOG;
    pub use crate::{context::AppContext, AppResult, CACHE_DIR, HOME_DIR, KUBE_DIR};
    pub use anyhow::Context;
}

use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader},
    ops::Deref,
    os::unix::prelude::ExitStatusExt,
    process::Stdio,
};
use std::{env, path::PathBuf};

use anyhow::Context;
use lazy_static::lazy_static;
use log::Log;
use structopt::StructOpt;

use configure::CmdConfigure;
use context::AppContext;
use deploy::CmdDeploy;
use flash::CmdFlash;
use hocfile::Hocfile;
use publish::CmdPublish;

lazy_static! {
    pub static ref HOME_DIR: PathBuf = PathBuf::from(format!("{}/.hoc", env::var("HOME").unwrap()));
    pub static ref CACHE_DIR: PathBuf = HOME_DIR.join("cache");
    pub static ref KUBE_DIR: PathBuf = HOME_DIR.join("kube");
    pub static ref LOG: Log = Log::new();
}

fn readable_size(size: usize) -> (f32, &'static str) {
    let mut order_10_bits = 0;
    let mut size = size as f32;
    while size >= 1024.0 && order_10_bits < 4 {
        size /= 1024.0;
        order_10_bits += 1;
    }

    let unit = match order_10_bits {
        0 => "bytes",
        1 => "KiB",
        2 => "MiB",
        3 => "GiB",
        4 => "TiB",
        _ => unreachable!(),
    };

    (size, unit)
}

pub type AppResult<T> = anyhow::Result<T>;

#[derive(StructOpt)]
struct App {
    /// Use cached image instead of fetching it.
    #[structopt(short, long)]
    cached: bool,

    #[structopt(flatten)]
    subcommand: Subcommand,
}

#[derive(StructOpt)]
enum Subcommand {
    Flash(CmdFlash),
    Configure(CmdConfigure),
    Publish(CmdPublish),
    Deploy(CmdDeploy),
}

async fn run() -> AppResult<()> {
    let hocfile = Hocfile::unvalidated_from_slice(include_bytes!("../Hocfile.default.yaml"))?;
    let optional_set_dependencies = hocfile.optional_set_dependencies();

    fn create_arg<'a>(
        name: &'a str,
        default: Option<&'a str>,
        required: bool,
    ) -> clap::Arg<'a, 'a> {
        let mut arg = clap::Arg::with_name(name);

        if required {
            arg = arg.required(true);
        } else {
            arg = arg.long(name);
        }

        if let Some(default) = default {
            arg = arg.default_value(default);
        } else {
            arg = arg.takes_value(true);
        }

        arg
    }

    fn cloned_arg_set<'a, 'b>(
        arg_ref: &str,
        args: &'a Vec<(&str, Vec<clap::Arg<'b, 'b>>)>,
    ) -> impl Iterator<Item = clap::Arg<'b, 'b>> + 'a {
        args.iter()
            .find(|(name, _)| *name == arg_ref)
            .unwrap()
            .1
            .iter()
            .cloned()
    }

    let mut app = clap::App::new("hoc");
    let mut optional_args = Vec::with_capacity(hocfile.optional_sets.len());

    for optional_set in optional_set_dependencies.nodes() {
        let mut optionals = Vec::new();
        for optional in optional_set.optionals.iter() {
            match optional {
                hocfile::Optional::Concrete(optional) => {
                    optionals.push(create_arg(
                        &optional.name,
                        optional.default.as_deref(),
                        false,
                    ));
                }
                hocfile::Optional::Set { from_optional_set } => {
                    optionals.extend(cloned_arg_set(from_optional_set.as_ref(), &optional_args))
                }
            }
        }
        optional_args.push((optional_set.name.deref(), optionals));
    }

    for command in hocfile.commands.iter() {
        let mut subcommand = clap::SubCommand::with_name(&command.name);

        for argument in command.arguments.iter() {
            subcommand = subcommand.arg(create_arg(&argument.name, None, true));
        }

        for optional in command.optionals.iter() {
            match optional {
                hocfile::Optional::Concrete(optional) => {
                    subcommand = subcommand.arg(create_arg(
                        &optional.name,
                        optional.default.as_deref(),
                        false,
                    ));
                }
                hocfile::Optional::Set { from_optional_set } => {
                    for arg in cloned_arg_set(from_optional_set.as_ref(), &optional_args) {
                        subcommand = subcommand.arg(arg);
                    }
                }
            }
        }

        app = app.subcommand(subcommand);
    }

    let matches = app.get_matches();
    if let (subcmd_name, Some(subcmd_matches)) = matches.subcommand() {
        use hocfile::{BuiltInFn, ProcedureStep};

        // Safety: We know the command exists, since we have successfully received matches from
        // clap.
        let command = hocfile.find_command(&subcmd_name).unwrap();

        let num_steps = command.procedure.len();
        for (step_i, step) in (1..).zip(&command.procedure) {
            let mut arguments: HashMap<_, _> = command
                .arguments()
                .flat_map(|arg| {
                    Some((arg.name.deref(), subcmd_matches.value_of(arg.name.deref())?))
                })
                .collect();
            let mut optionals: HashMap<_, _> = command
                .optionals(&hocfile)
                .flat_map(|optional| {
                    Some((
                        optional.name.deref(),
                        subcmd_matches
                            .value_of(optional.name.deref())
                            .or(optional.default.as_deref())?,
                    ))
                })
                .collect();

            let script_proc = match step {
                ProcedureStep::BuiltIn(built_in_fn) => {
                    status!(
                        "Step {}/{}: [built-in] {:?}",
                        step_i,
                        num_steps,
                        built_in_fn
                    );

                    match built_in_fn {
                        BuiltInFn::DockerBuild => {
                            let cmd_build = crate::build::FnDockerBuild {
                                service: arguments.remove("service").unwrap(),
                                branch: optionals.remove("branch").unwrap(),
                            };

                            cmd_build.run().await?;
                        }
                    }

                    continue;
                }

                ProcedureStep::FromScript(script_ref) => {
                    let script = hocfile.find_script(&script_ref).unwrap();
                    &script.source
                }

                ProcedureStep::Script(script) => script,
            };

            status!(
                "Step {}/{}: {}",
                step_i,
                num_steps,
                if let ProcedureStep::FromScript(script_ref) = step {
                    format!("[script] {}", script_ref.deref())
                } else {
                    "[inline] Custom step".to_string()
                },
            );

            let exit_status = {
                let mut child = std::process::Command::new("bash")
                    .args(&["-eu", "-c"])
                    .arg(script_proc)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()?;

                status!("Script output");
                let stdout = child.stdout.take();
                let stderr = child.stderr.take();
                let stdout_handle = std::thread::spawn(|| -> io::Result<()> {
                    if let Some(stdout) = stdout {
                        let reader = BufReader::new(stdout);
                        for line in reader.lines() {
                            info!(line?);
                        }
                    }
                    Ok(())
                });
                let stderr_handle = std::thread::spawn(|| -> io::Result<()> {
                    if let Some(stderr) = stderr {
                        let reader = BufReader::new(stderr);
                        for line in reader.lines() {
                            error!(line?);
                        }
                    }
                    Ok(())
                });

                stdout_handle.join().unwrap()?;
                stderr_handle.join().unwrap()?;
                child.wait()?
            };

            if exit_status.success() {
                continue;
            }

            if let Some(code) = exit_status.code() {
                error!("Script exited with status {}.", code);
            } else if let Some(signal) = exit_status.signal() {
                error!("Script was interupted by signal code {}.", signal);
            } else {
                error!("Script failed.");
            }

            anyhow::bail!("Command '{}' failed", command.name.deref());
        }
    }

    let args = App::from_args();
    let mut context = AppContext::configure(args.cached).context("Configuring app context")?;

    match args.subcommand {
        Subcommand::Flash(cmd) => cmd.run(&mut context).await.context("flash command"),
        Subcommand::Configure(cmd) => cmd.run(&mut context).await.context("configure command"),
        Subcommand::Publish(cmd) => cmd.run().await.context("publish command"),
        Subcommand::Deploy(cmd) => cmd.run().await.context("deploy command"),
    }
}

#[tokio::main]
async fn main() {
    match run().await {
        Err(e) => error!(e
            .chain()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(": ")),
        _ => (),
    }
}
