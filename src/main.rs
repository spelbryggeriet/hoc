#[macro_use]
extern crate log;
#[macro_use]
extern crate strum_macros;

mod context;
mod logger;
mod file;

mod build;
mod deploy;
mod flash;

mod prelude;

use std::{env, path::PathBuf};

use anyhow::Context;
use lazy_static::lazy_static;
use logger::Logger;
use structopt::StructOpt;

use build::CmdBuild;
use context::AppContext;
use deploy::CmdDeploy;
use flash::CmdFlash;

lazy_static! {
    static ref HOME_DIR: PathBuf = {
        let mut home_dir = PathBuf::new();
        home_dir.push(env::var("HOME").expect("HOME not set"));
        home_dir.push(".h2t");
        home_dir
    };
    static ref CACHE_DIR: PathBuf = HOME_DIR.join("cache");
}

fn readable_size(size: usize) -> (f32, &'static str) {
    let mut order_10_bits = 0;
    let mut size = size as f32;
    while size >= 1024.0 && order_10_bits < 4 {
        size /= 1024.0;
        order_10_bits += 1;
    }

    let unit = match order_10_bits {
        0 => "bytes",
        1 => "KiB",
        2 => "MiB",
        3 => "GiB",
        4 => "TiB",
        _ => unreachable!(),
    };

    (size, unit)
}

type AppResult<T> = anyhow::Result<T>;

#[derive(StructOpt)]
struct App {
    /// Use cached image instead of fetching it.
    #[structopt(short, long)]
    cached: bool,

    #[structopt(flatten)]
    subcommand: Subcommand,
}

#[derive(StructOpt)]
enum Subcommand {
    Build(CmdBuild),
    Deploy(CmdDeploy),
    Flash(CmdFlash),
}

async fn run(log: &mut Logger) -> AppResult<()> {
    pretty_env_logger::init();

    let args = App::from_args();
    let mut context = AppContext::configure(args.cached).context("Configuring app context")?;

    match args.subcommand {
        Subcommand::Build(cmd) => cmd.run(log).await.context("Running build command")?,
        Subcommand::Deploy(cmd) => cmd.run(log).await.context("Running deploy command")?,
        Subcommand::Flash(cmd) => cmd
            .run(&mut context, log)
            .await
            .context("Running flash command")?,
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    let mut log = Logger::new();

    match run(&mut log).await {
        Err(e) => log
            .error(
                e.chain()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(": "),
            )
            .expect("Failed writing error log"),
        _ => (),
    }
}
