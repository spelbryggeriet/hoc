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

        debug!("Feching HOME environment variable");
        let home_dir = env::var("HOME")?;

        debug!("Loading context");
        let context = Context::load(format!("{home_dir}/.config/hoc/context.yaml"))?;

        match app.command {
            Command::Init(init_command) => {
                debug!("Running {} command", init::Command::command().get_name());
                init_command.run(context)?;
            }
            _ => (),
        }
    }
}

#[derive(Subcommand)]
enum Command {
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

    let mut rng = <rand_chacha::ChaCha8Rng as rand::SeedableRng>::seed_from_u64(2);
    let mut progresses = Vec::new();

    for i in 1.. {
        let n = rand::Rng::gen_range(&mut rng, 0..6);

        if n == 0 {
            trace!("Trace {i}");
        } else if n == 1 {
            debug!("Debug {i}");
        } else if n == 2 {
            info!("Info {i}");
        } else if n == 3 {
            warn!("Warning {i}");
        } else if n == 4 {
            error!("Error {i}");
        } else {
            progresses.push((
                progress!("Progress {i}"),
                if rand::Rng::gen_ratio(&mut rng, 99, 100) {
                    rand::Rng::gen_range(&mut rng, 50..100)
                } else {
                    rand::Rng::gen_range(&mut rng, 0..5)
                },
            ));
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

    logger::Logger::cleanup()?;
    return ExitCode::from(1);

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
