use clap::Parser;
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
        let app = Self::from_args();
        let context = Context::new("~/.config/hoc/context.yaml");
        match app.command {
            Command::Init(init_command) => init_command.run(context)?,
            _ => (),
        }
    }
}

#[derive(Parser)]
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
