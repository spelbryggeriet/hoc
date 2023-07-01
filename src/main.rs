use std::{
    env,
    path::{Path, PathBuf},
    process::ExitCode,
};

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
mod util;

fn home_dir() -> PathBuf {
    let home_dir = env::var("HOME").expect(EXPECT_HOME_ENV_VAR);
    PathBuf::from(home_dir)
}

fn local_context_file_path() -> PathBuf {
    home_dir().join(".local/share/hoc/context.yaml")
}

fn local_files_dir() -> PathBuf {
    home_dir().join(".local/share/hoc/files")
}

fn local_cache_dir() -> PathBuf {
    home_dir().join(".cache/hoc/cache")
}

fn local_temp_dir() -> PathBuf {
    home_dir().join(".cache/hoc/temp")
}

fn local_source_dir() -> PathBuf {
    home_dir().join(".cache/hoc/source")
}

fn container_files_dir() -> &'static Path {
    Path::new("/hoc/files")
}

fn container_cache_dir() -> &'static Path {
    Path::new("/hoc/cache")
}

fn container_temp_dir() -> &'static Path {
    Path::new("/hoc/temp")
}

fn container_source_dir() -> &'static Path {
    Path::new("/hoc/source")
}

fn remote_temp_dir() -> &'static Path {
    Path::new("/tmp/hoc")
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
    Context::get_or_init().load()?;

    defer! {
        if let Err(err) = Context::get_or_init().persist() {
            error!("{err}");
        }

        if let Err(err) = Context::get_or_init().cleanup() {
            error!("{err}");
        }

        if let Err(err) = log::cleanup() {
            eprintln!("{err}");
        }
    }

    let exit_code = match app.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error!("{error:?}");
            ExitCode::FAILURE
        }
    };

    exit_code
}
