use std::{env, process::ExitCode};

use clap::{CommandFactory, Parser, Subcommand};
use context::Context;

#[macro_use]
mod macros;

mod cidr;
mod context;
mod init;
mod logger;
mod prelude;
mod prompt;

use prelude::*;

#[derive(Parser)]
struct App {
    #[clap(subcommand)]
    command: Command,
}

impl App {
    #[throws(anyhow::Error)]
    fn run() {
        debug!("Parsing command-line arguments");
        let app = Self::from_args();

        #[cfg(debug_assertions)]
        if matches!(app.command, Command::Debug) {
            run_debug();
            return;
        }

        debug!("Feching HOME environment variable");
        let home_dir = env::var("HOME")?;

        debug!("Loading context");
        let mut context = Context::load(format!("{home_dir}/.config/hoc/context.yaml"))?;

        match app.command {
            Command::Init(init_command) => {
                debug!("Running {} command", init::Command::command().get_name());
                init_command.run(&mut context)?;
            }
            _ => (),
        }
    }
}

#[derive(Subcommand)]
enum Command {
    #[cfg(debug_assertions)]
    Debug,

    Deploy(DeployCommand),
    Init(init::Command),
    Node(NodeCommand),
    SdCard(SdCardCommand),
}

/// Deploy an application
#[derive(Parser)]
struct DeployCommand {}

/// Manage a node
#[derive(Parser)]
struct NodeCommand {}

/// Manage an SD card
#[derive(Parser)]
struct SdCardCommand {}

#[throws(anyhow::Error)]
fn main() -> ExitCode {
    logger::Logger::init()?;

    let exit_code = match App::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error!("{error}");
            ExitCode::from(1)
        }
    };

    logger::Logger::cleanup()?;

    exit_code
}

#[cfg(debug_assertions)]
fn run_debug() {
    let mut rng = <rand_chacha::ChaCha8Rng as rand::SeedableRng>::seed_from_u64(2);
    let mut progresses = Vec::<(_, i32)>::new();

    for i in 0.. {
        let d = progresses.len();

        if rand::Rng::gen_ratio(&mut rng, 1, 5) {
            let ttl = if rand::Rng::gen_ratio(&mut rng, 1, 20) {
                rand::Rng::gen_range(&mut rng, 50..100)
            } else {
                rand::Rng::gen_range(&mut rng, 0..5)
            };

            progresses.push((progress!("Progress {}-{i}", d + 1), ttl));
            progresses.iter_mut().rev().fold(0, |max, (_, ttl)| {
                if *ttl <= max {
                    *ttl = max + 1;
                }
                *ttl
            });
        } else {
            if rand::Rng::gen_ratio(&mut rng, 1, 2) {
                trace!("Trace {d}-{i}");
            } else if rand::Rng::gen_ratio(&mut rng, 1, 2) {
                debug!("Debug {d}-{i}");
            } else if rand::Rng::gen_ratio(&mut rng, 9, 10) {
                info!("Info {d}-{i}");
            } else if rand::Rng::gen_ratio(&mut rng, 1, 2) {
                warn!("Warning {d}-{i}");
            } else {
                error!("Error {d}-{i}");
            }
        }

        progresses.retain_mut(|(_, ttl)| {
            if *ttl == 0 {
                false
            } else {
                *ttl -= 1;
                true
            }
        });

        if rand::Rng::gen_ratio(&mut rng, 3, 4) {
            std::thread::sleep(std::time::Duration::from_millis(rand::Rng::gen_range(
                &mut rng,
                5..50,
            )));
        } else {
            std::thread::sleep(std::time::Duration::from_millis(rand::Rng::gen_range(
                &mut rng,
                100..1000,
            )));
        }
    }
}
