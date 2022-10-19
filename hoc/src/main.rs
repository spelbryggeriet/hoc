use std::{env, process::ExitCode};

use clap::Parser;
use scopeguard::defer;

use action::Action;
use context::Context;

#[macro_use]
mod macros;

mod action;
mod cidr;
mod context;
mod logger;
mod prelude;
mod prompt;
mod util;

use prelude::*;

#[derive(Parser)]
struct App {
    #[clap(subcommand)]
    action: Action,
}

impl App {
    #[throws(anyhow::Error)]
    async fn run(self) {
        #[cfg(debug_assertions)]
        if matches!(self.action, Action::Debug) {
            action::debug::run();
            return;
        }

        debug!("Feching HOME environment variable");
        let home_dir = env::var("HOME")?;

        let context = Context::load(format!("{home_dir}/.config/hoc/context.yaml"))?;
        context::CONTEXT
            .set(context)
            .unwrap_or_else(|_| panic!("context already initialized"));

        defer! {
            context::CONTEXT
                .get()
                .expect(EXPECT_CONTEXT_INITIALIZED)
                .persist().unwrap_or_else(|err| panic!("{err}"));
        }

        self.action.run().await?;
    }
}

#[throws(anyhow::Error)]
#[async_std::main]
async fn main() -> ExitCode {
    let app = App::from_args();

    logger::Logger::init()?;

    defer! {
        logger::Logger::cleanup().unwrap_or_else(|err| panic!("{err}"));
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
