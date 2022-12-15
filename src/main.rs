use std::{env, path::PathBuf, process::ExitCode};

use anyhow::Error;
use clap::Parser;
use scopeguard::defer;

use self::{command::Command, context::Context, ledger::Ledger, prelude::*};

#[macro_use]
mod macros;

mod cidr;
mod command;
mod context;
mod ledger;
mod log;
mod prelude;
mod process;
mod prompt;
mod temp;
mod util;

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
    fn run(self) {
        match self.command.run() {
            Ok(()) => (),
            Err(err) => {
                error!("{err}");
                Ledger::get_or_init().rollback()?;
            }
        }
    }
}

#[throws(Error)]
fn main() -> ExitCode {
    let app = App::parse();

    log::init()?;

    defer! {
        debug!("Cleaning temporary files");
        if let Err(err) = temp::clean() {
            error!("{err}");
        }

        if let Err(err) = log::cleanup() {
            eprintln!("{err}");
        }
    }

    let res = if app.command.needs_context() {
        Context::get_or_init().load()?;

        defer! {
            if let Err(err) = Context::get_or_init().persist() {
                error!("{err}");
            }
        };
        app.run()
    } else {
        app.run()
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
