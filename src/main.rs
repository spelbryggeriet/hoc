#[macro_use]
extern crate log;
#[macro_use]
extern crate strum_macros;

mod logger;

mod build;
mod deploy;
mod flash;

mod prelude;

use std::path::PathBuf;
use std::{env, fs};

use anyhow::Context;
use lazy_static::lazy_static;
use logger::Logger;
use structopt::StructOpt;

use build::CmdBuild;
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

fn configure_home_dir(log: &mut Logger) -> AppResult<()> {
    if !is_home_dir_complete() {
        log.status(format!(
            "Configuring home directory at '{}'",
            HOME_DIR.to_string_lossy()
        ))?;
    }

    fs::create_dir_all(HOME_DIR.join("cache")).context("Creating cache directory")?;

    Ok(())
}

fn is_home_dir_complete() -> bool {
    CACHE_DIR.exists()
}

fn readable_size(size: u64) -> (f32, &'static str) {
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
enum App {
    Build(CmdBuild),
    Deploy(CmdDeploy),
    Flash(CmdFlash),
}

async fn run(log: &mut Logger) -> AppResult<()> {
    match App::from_args() {
        App::Build(cmd) => cmd.run(log).await.context("Running build command")?,
        App::Deploy(cmd) => cmd.run(log).await.context("Running deploy command")?,
        App::Flash(cmd) => cmd.run(log).await.context("Running flash command")?,
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    pretty_env_logger::init();

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
