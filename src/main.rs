#[macro_use]
extern crate strum_macros;

macro_rules! _log {
    ([$label:literal, $method:ident]) => {
        crate::LOG.lock()
            .unwrap()
            .$method("")
            .unwrap_or_else(|e| panic!(concat!("failed printing to ", $label, ": {}"), e))
    };

    ([$label:literal, $method:ident] $msg:expr $(,)?) => {
        crate::LOG.lock()
            .unwrap()
            .$method(&$msg)
            .unwrap_or_else(|e| panic!(concat!("failed printing to ", $label, ": {}"), e))
    };

    ([$label:literal, $method:ident] $template:literal, $($args:tt)+) => {
        crate::LOG.lock()
            .unwrap()
            .$method(format!($template, $($args)+))
            .unwrap_or_else(|e| panic!(concat!("failed printing to ", $label, ": {}"), e))
    };
}

macro_rules! info {
    ($($args:tt)*) => {
        _log!(["stdout", info] $($args)*)
    };
}

macro_rules! status {
    ($($args:tt)*) => {
        _log!(["stdout", status] $($args)*)
    };
}

macro_rules! error {
    ($($args:tt)*) => {
        _log!(["stderr", error] $($args)*)
    };
}

macro_rules! prompt {
    ($($args:tt)*) => {
        _log!(["stdout", prompt] $($args)*)
    };
}

/// Ask for user input.
///
/// # Examples
///
/// ```rust
/// let name     = input!("Give me your name");
/// let password = input!([hidden] "Give me your password");
/// ```
macro_rules! input {
    ([hidden] $($args:tt)*) => {
        _log!(["stdout", hidden_input] $($args)*)
    };

    ($($args:tt)*) => {
        _log!(["stdout", input] $($args)*)
    };
}

macro_rules! choose {
    ($msg:expr, $items:expr $(, $default_index:expr)? $(,)?) => {
        crate::LOG.lock()
            .unwrap()
            .choose($msg, $items, $( if true { $default_index } else )? { 0 })
            .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e))
    };
}

mod context;
mod file;
mod logger;
mod service;

mod build;
mod configure;
mod deploy;
mod flash;
mod publish;

mod prelude {
    pub use anyhow::Context;
    pub use crate::file::{NamedFile, TempDir};
    pub use crate::{context::AppContext, AppResult, CACHE_DIR, HOME_DIR};
}

use std::sync::Mutex;
use std::{env, path::PathBuf};

use anyhow::Context;
use lazy_static::lazy_static;
use logger::Logger;
use structopt::StructOpt;

use build::CmdBuild;
use configure::CmdConfigure;
use context::AppContext;
use deploy::CmdDeploy;
use flash::CmdFlash;
use publish::CmdPublish;

lazy_static! {
    pub static ref HOME_DIR: PathBuf = PathBuf::from(format!("{}/.h2t", env::var("HOME").unwrap()));
    pub static ref CACHE_DIR: PathBuf = HOME_DIR.join("cache");
    pub static ref LOG: Mutex<Logger> = Mutex::new(Logger::new());
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

pub type AppResult<T> = anyhow::Result<T>;

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
    Flash(CmdFlash),
    Configure(CmdConfigure),
    Build(CmdBuild),
    Publish(CmdPublish),
    Deploy(CmdDeploy),
}

async fn run() -> AppResult<()> {
    let args = App::from_args();
    let mut context = AppContext::configure(args.cached).context("Configuring app context")?;

    match args.subcommand {
        Subcommand::Flash(cmd) => cmd.run(&mut context).await.context("flash command"),
        Subcommand::Configure(cmd) => cmd.run(&mut context).await.context("configure command"),
        Subcommand::Build(cmd) => cmd.run().await.context("build command"),
        Subcommand::Publish(cmd) => cmd.run().await.context("publish command"),
        Subcommand::Deploy(cmd) => cmd.run().await.context("deploy command"),
    }
}

#[tokio::main]
async fn main() {
    match run().await {
        Err(e) => error!(e
            .chain()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(": ")),
        _ => (),
    }
}
