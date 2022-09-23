use std::env;

use clap::{CommandFactory, Parser, Subcommand};
use context::Context;
use env_logger::Env;

#[macro_use]
mod macros;

mod cidr;
mod context;
mod init;
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
        debug!("parsing command-line arguments");
        let app = Self::from_args();

        debug!("feching HOME environment variable");
        let home_dir = env::var("HOME")?;

        debug!("loading context");
        let context = Context::load(format!("{home_dir}/.config/hoc/context.yaml"))?;

        match app.command {
            Command::Init(init_command) => {
                info!("running {} command", init::Command::command().get_name());
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

const LOWEST_DEFAULT_LEVEL: &'static str = if cfg!(debug_assertions) {
    "debug"
} else {
    "info"
};

#[throws(anyhow::Error)]
fn main() {
    env_logger::Builder::from_env(Env::default().default_filter_or(LOWEST_DEFAULT_LEVEL)).init();
    App::run()?;
}
