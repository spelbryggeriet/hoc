use std::{env, process::ExitCode};

use clap::Parser;
use scopeguard::defer;

#[macro_use]
mod macros;

mod cidr;
mod command;
mod context;
mod log;
mod prelude;
mod prompt;
mod runner;
mod util;

use command::Command;
use prelude::*;

#[derive(Parser)]
struct App {
    #[clap(subcommand)]
    command: Command,
}

impl App {
    #[throws(anyhow::Error)]
    async fn run(self) {
        debug!("Feching HOME environment variable");
        let home_dir = env::var("HOME")?;

        context::init(
            format!("{home_dir}/.local/share/hoc"),
            format!("{home_dir}/.cache/hoc"),
        )
        .await?;

        defer! {
            if let Err(err) = context::get_context().persist() {
                error!("{err}");
                return;
            }
        }

        self.command.run().await?;
    }
}

#[throws(anyhow::Error)]
#[tokio::main]
async fn main() -> ExitCode {
    let app = App::from_args();

    log::init()?;

    defer! {
        if let Err(err) = log::cleanup() {
            eprintln!("{err}");
            return;
        }
    }

    let exit_code = match app.run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error!("{error:?}");
            ExitCode::FAILURE
        }
    };

    exit_code
}
