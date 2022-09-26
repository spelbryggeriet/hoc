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
    match App::run() {
        Ok(()) => {
            log::logger().flush();
            ExitCode::SUCCESS
        }
        Err(error) => {
            error!("{error}");
            log::logger().flush();
            ExitCode::from(1)
        }
    }
}
