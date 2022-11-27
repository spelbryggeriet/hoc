use std::{env, path::PathBuf, process::ExitCode};

use anyhow::Error;
use clap::Parser;
use scopeguard::defer;
use tokio::{runtime::Handle, task};

use self::{command::Command, context::Context, ledger::Ledger, prelude::*};

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

const EXPECT_HOME_ENV_VAR: &str = "HOME environment variable not set";

fn data_dir() -> PathBuf {
    let home_dir = env::var("HOME").expect(EXPECT_HOME_ENV_VAR);
    PathBuf::from(format!("{home_dir}/.local/share/hoc"))
}

fn cache_dir() -> PathBuf {
    let home_dir = env::var("HOME").expect(EXPECT_HOME_ENV_VAR);
    PathBuf::from(format!("{home_dir}/.cache/hoc"))
}

#[derive(Parser)]
struct App {
    #[clap(subcommand)]
    command: Command,
}

impl App {
    #[throws(Error)]
    async fn run(self) {
        match self.command.run().await {
            Ok(()) => (),
            Err(err) => {
                error!("{err}");
                Ledger::get_or_init().lock().await.rollback().await?;
            }
        }
    }
}

#[throws(Error)]
#[tokio::main]
async fn main() -> ExitCode {
    let app = App::parse();

    log::init()?;

    defer! {
        if let Err(err) = log::cleanup() {
            eprintln!("{err}");
        }
    }

    Context::get_or_init().load().await?;

    let res = if app.command.needs_context() {
        defer! {
            task::block_in_place(|| {
                Handle::current().block_on(async {
                    if let Err(err) = Context::get_or_init().persist().await {
                        error!("{err}");
                    }
                });
            });
        };
        app.run().await
    } else {
        app.run().await
    };

    let exit_code = match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error!("{error:?}");
            ExitCode::FAILURE
        }
    };

    exit_code
}
