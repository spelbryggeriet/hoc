use std::{env, process::ExitCode};

use clap::Parser;
use futures::StreamExt;
use scopeguard::defer;
use tokio::pin;

use self::{command::Command, ledger::Ledger};
use prelude::*;

#[macro_use]
mod macros;

mod cidr;
mod command;
mod context;
mod ledger;
mod log;
mod prelude;
mod prompt;
mod runner;
mod util;

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

        match self.command.run().await {
            Ok(()) => (),
            Err(err) => {
                error!("{err}");

                let yes = "Yes";
                let opt = select!("Do you want to roll back the changes?")
                    .with_options([yes, "No"])
                    .get()?;

                if opt == yes {
                    progress!("Rolling back changes");

                    let mut ledger = Ledger::get_or_init().lock().await;
                    let stream = ledger.rollback();
                    pin!(stream);
                    while let Some(()) = stream.next().await {}
                }
            }
        }
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
