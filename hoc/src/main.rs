use std::{env, process::ExitCode};

use clap::Parser;
use scopeguard::defer;

#[macro_use]
mod macros;

mod cidr;
mod context;
mod log;
mod prelude;
mod prompt;
mod runner;
mod subcommand;
mod util;

use prelude::*;
use subcommand::Subcommand;

#[derive(Parser)]
struct App {
    #[clap(subcommand)]
    subcommand: Subcommand,
}

impl App {
    #[throws(anyhow::Error)]
    async fn run(self) {
        #[cfg(debug_assertions)]
        if matches!(self.subcommand, Subcommand::Debug) {
            subcommand::debug::run();
            return;
        }

        debug!("Feching HOME environment variable");
        let home_dir = env::var("HOME")?;

        context::init(
            format!("{home_dir}/.local/share/hoc"),
            format!("{home_dir}/.cache/hoc"),
        )?;

        defer! {
            if let Err(err) = context::get_context().persist() {
                error!("{err}");
                return;
            }
        }

        self.subcommand.run().await?;
    }
}

#[throws(anyhow::Error)]
#[async_std::main]
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
            error!("{error}");
            ExitCode::FAILURE
        }
    };

    exit_code
}
