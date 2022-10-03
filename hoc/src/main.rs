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
    use crossterm::ExecutableCommand;

    logger::Logger::init()?;

    {
        info!("Test");
        let progress = progress!("Progress");
        for i in 0..10 {
            std::thread::sleep(std::time::Duration::new(0, 100_000_000));
            info!("Info {i}")
        }
        for i in 0..50 {
            std::thread::sleep(std::time::Duration::new(0, 10_000_000));
            info!("Info {i}")
        }
        let progress2 = progress!("Progress 2");
        for i in 0..10 {
            std::thread::sleep(std::time::Duration::new(0, 100_000_000));
            info!("Info {i}")
        }
        for i in 0..50 {
            std::thread::sleep(std::time::Duration::new(0, 10_000_000));
            info!("Info {i}")
        }
        let progress3 = progress!("Progress 3");
        for i in 0..10 {
            std::thread::sleep(std::time::Duration::new(0, 100_000_000));
            info!("Info {i}")
        }
        for i in 0..50 {
            std::thread::sleep(std::time::Duration::new(0, 10_000_000));
            info!("Info {i}")
        }
        let progress4 = progress!("Progress 4");
        for i in 0..10 {
            std::thread::sleep(std::time::Duration::new(0, 100_000_000));
            info!("Info {i}")
        }
        for i in 0..50 {
            std::thread::sleep(std::time::Duration::new(0, 10_000_000));
            info!("Info {i}")
        }
        let progress5 = progress!("Progress 5");
        for i in 0..10 {
            std::thread::sleep(std::time::Duration::new(0, 100_000_000));
            info!("Info {i}")
        }
        for i in 0..50 {
            std::thread::sleep(std::time::Duration::new(0, 10_000_000));
            info!("Info {i}")
        }
        let progress6 = progress!("Progress 6");
        for i in 0..10 {
            std::thread::sleep(std::time::Duration::new(0, 100_000_000));
            info!("Info {i}")
        }
        for i in 0..50 {
            std::thread::sleep(std::time::Duration::new(0, 10_000_000));
            info!("Info {i}")
        }
        let progress7 = progress!("Progress 7");
        for i in 0..10 {
            std::thread::sleep(std::time::Duration::new(0, 100_000_000));
            info!("Info {i}")
        }
        for i in 0..50 {
            std::thread::sleep(std::time::Duration::new(0, 10_000_000));
            info!("Info {i}")
        }
        let progress8 = progress!("Progress 8");
        for i in 0..10 {
            std::thread::sleep(std::time::Duration::new(0, 100_000_000));
            info!("Info {i}")
        }
        for i in 0..50 {
            std::thread::sleep(std::time::Duration::new(0, 10_000_000));
            info!("Info {i}")
        }
        let progress8 = progress!("Progress 8");
        for i in 0..10 {
            std::thread::sleep(std::time::Duration::new(0, 100_000_000));
            info!("Info {i}")
        }
        for i in 0..50 {
            std::thread::sleep(std::time::Duration::new(0, 10_000_000));
            info!("Info {i}")
        }
        let progress9 = progress!("Progress 9");
        for i in 0..10 {
            std::thread::sleep(std::time::Duration::new(0, 100_000_000));
            info!("Info {i}")
        }
        for i in 0..50 {
            std::thread::sleep(std::time::Duration::new(0, 10_000_000));
            info!("Info {i}")
        }
        let progress10 = progress!("Progress 10");
        for i in 0..10 {
            std::thread::sleep(std::time::Duration::new(0, 100_000_000));
            info!("Info {i}")
        }
        for i in 0..50 {
            std::thread::sleep(std::time::Duration::new(0, 10_000_000));
            info!("Info {i}")
        }
        std::thread::sleep(std::time::Duration::new(10, 0));
    }

    logger::Logger::cleanup()?;
    return ExitCode::from(0);

    let exit_code = match App::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error!("{error}");
            ExitCode::from(1)
        }
    };

    logger::Logger::cleanup()?;

    exit_code
}
