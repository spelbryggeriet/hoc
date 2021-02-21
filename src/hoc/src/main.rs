#[macro_use]
extern crate strum_macros;

#[macro_use]
extern crate hoclog;

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
    pub use crate::LOG;
    pub use crate::{context::AppContext, AppResult, CACHE_DIR, HOME_DIR, KUBE_DIR};
    pub use anyhow::Context;
    pub use hoclog::{Styling, Wrapping, status, info};
}

use std::{
    collections::HashMap,
    ffi::{CString, OsString},
    fs,
    io::{self, BufRead, BufReader},
    ops::Deref,
    process::Stdio,
};
use std::{env, path::PathBuf};

#[cfg(target_family = "unix")]
use std::os::unix::{ffi::OsStrExt, prelude::ExitStatusExt};

use anyhow::Context;
use lazy_static::lazy_static;
use rand::{distributions::Alphanumeric, Rng};
use structopt::StructOpt;
use tera::Tera;

use configure::CmdConfigure;
use context::AppContext;
use hocfile::{HocValue, Hocfile};
use hoclog::Log;
use thiserror::private::PathAsDisplay;

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

struct TempPipe {
    path: PathBuf,
}

impl TempPipe {
    fn new(mode: u32) -> io::Result<Self> {
        const RAND_LEN: usize = 10;

        let mut buf = env::temp_dir();
        let mut name = OsString::with_capacity(3 + RAND_LEN);
        name.push("tmp");

        unsafe {
            rand::thread_rng()
                .sample_iter(&Alphanumeric)
                .take(RAND_LEN)
                .for_each(|b| name.push(std::str::from_utf8_unchecked(&[b as u8])))
        }

        buf.push(name);

        let path = CString::new(buf.as_os_str().as_bytes())?;
        let result: libc::c_int = unsafe { libc::mkfifo(path.as_ptr(), mode as libc::mode_t) };

        let result: i32 = result.into();
        if result == 0 {
            return Ok(TempPipe { path: buf });
        }

        let error = errno::errno();
        let kind = match error.0 {
            libc::EACCES => io::ErrorKind::PermissionDenied,
            libc::EEXIST => io::ErrorKind::AlreadyExists,
            libc::ENOENT => io::ErrorKind::NotFound,
            _ => io::ErrorKind::Other,
        };

        Err(io::Error::new(
            kind,
            format!("could not open {:?}: {}", path, error),
        ))
    }
}

impl Drop for TempPipe {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
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
    Configure(CmdConfigure),
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

        let sync_pipe = TempPipe::new(0o644)?;
        println!("{}", sync_pipe.path.as_display());

        // Safety: We know the command exists, since we have successfully received matches from
        // clap.
        let command = hocfile.find_command(&subcmd_name).unwrap();

        let mut input: HashMap<_, _> = command
            .arguments()
            .flat_map(|arg| {
                Some((
                    arg.name.to_string(),
                    HocValue::String(subcmd_matches.value_of(arg.name.deref())?.to_string()),
                ))
            })
            .collect();
        input.extend(command.optionals(&hocfile).flat_map(|optional| {
            Some((
                optional.name.to_string(),
                HocValue::String(
                    subcmd_matches
                        .value_of(optional.name.deref())
                        .or(optional.default.as_deref())?
                        .to_string(),
                ),
            ))
        }));
        input.insert(
            "hoc_pipe".into(),
            HocValue::String(
                sync_pipe
                    .path
                    .as_os_str()
                    .to_str()
                    .context("convertion OS string to Rust string")?
                    .to_string(),
            ),
        );

        let mut tera = Tera::default();
        tera.register_filter("squote", |value: &serde_json::Value, _: &_| {
            Ok(serde_json::Value::String(format!(
                "'{}'",
                value
                    .as_str()
                    .ok_or(tera::Error::msg("expected string"))?
                    .replace("'", r#"'\''"#)
            )))
        });

        let num_steps = command.procedure.len();
        for (step_i, step) in (1..).zip(&command.procedure) {
            let script_proc = match step {
                ProcedureStep::BuiltIn(built_in_fn) => {
                    status!(
                        "Step {}/{}: [built-in] {:?}",
                        step_i,
                        num_steps,
                        built_in_fn
                    );

                    match built_in_fn {
                        BuiltInFn::RpiFlash => {
                            let cached = input
                                .remove("cached")
                                .and_then(|s| s.as_string().ok())
                                .and_then(|s| s.parse().ok())
                                .unwrap();
                            let mut context =
                                AppContext::configure(cached).context("Configuring app context")?;
                            let cmd_flash = crate::flash::FnFlashRpi {};

                            cmd_flash.run(&mut context).await?;
                        }

                        BuiltInFn::DockerBuild => {
                            let cmd_build = crate::build::FnDockerBuild {
                                service: input
                                    .remove("service")
                                    .and_then(|s| s.as_string().ok())
                                    .unwrap(),
                                branch: input
                                    .remove("branch")
                                    .and_then(|s| s.as_string().ok())
                                    .unwrap(),
                            };

                            cmd_build.run().await?;
                        }

                        BuiltInFn::GitlabPublish => {
                            let cmd_publish = crate::publish::FnGitlabPublish {
                                service: input
                                    .remove("service")
                                    .and_then(|s| s.as_string().ok())
                                    .unwrap(),
                                branch: input
                                    .remove("branch")
                                    .and_then(|s| s.as_string().ok())
                                    .unwrap(),
                            };

                            cmd_publish.run().await?;
                        }

                        BuiltInFn::K8sDeploy => {
                            let cmd_deploy = crate::deploy::FnK8sDeploy {
                                service: input
                                    .remove("service")
                                    .and_then(|s| s.as_string().ok())
                                    .unwrap(),
                                branch: input
                                    .remove("branch")
                                    .and_then(|s| s.as_string().ok())
                                    .unwrap(),
                            };

                            cmd_deploy.run().await?;
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

            let (exit_status, output) = {
                let context = tera::Context::from_serialize(&input).unwrap();

                let template_script = hocfile.script.profile.clone() + &script_proc;
                let rendered_script = tera.render_str(&template_script, &context)?;

                let mut child = std::process::Command::new("bash")
                    .args(&["-eu", "-c"])
                    .arg(rendered_script)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()?;

                status!("Script output");
                let stdout = child.stdout.take();
                let stderr = child.stderr.take();

                let stderr_handle = std::thread::spawn(|| -> io::Result<()> {
                    if let Some(stderr) = stderr {
                        let reader = BufReader::new(stderr);
                        for line in reader.lines() {
                            error!(line?);
                        }
                    }
                    Ok(())
                });

                let mut output = HashMap::new();
                if let Some(stdout) = stdout {
                    let reader = BufReader::new(stdout);
                    for line in reader.lines() {
                        let line = line?;
                        info!(line);

                        hocfile::parse_hoc_line(&mut input, &mut output, &line)?;
                    }
                }

                stderr_handle.join().unwrap()?;
                (child.wait()?, output)
            };

            if exit_status.success() {
                #[cfg(debug_assertions)]
                for (key, value) in output.iter() {
                    let mut stack = vec![(false, value.clone())];
                    let mut debug = String::new();
                    debug += &key;
                    debug += ": ";

                    while let Some(item) = stack.pop() {
                        match item.1 {
                            HocValue::String(s) => {
                                if item.0 {
                                    debug += &s;
                                } else {
                                    debug.push('\'');
                                    debug += &s;
                                    debug.push('\'');
                                }
                            }
                            HocValue::List(l) => {
                                debug.push('[');
                                stack.push((true, HocValue::String("]".to_string())));
                                stack.extend(
                                    l.into_iter()
                                        .rev()
                                        .map(|item| {
                                            vec![
                                                (true, HocValue::String(",".to_string())),
                                                (false, item),
                                            ]
                                        })
                                        .flat_map(|list| list.into_iter())
                                        .skip(1),
                                );
                            }
                        }
                    }
                    info!("[DEBUG] {}", debug);
                }

                input.extend(output);

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
        Subcommand::Configure(cmd) => cmd.run(&mut context).await.context("configure command"),
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
