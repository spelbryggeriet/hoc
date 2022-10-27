use std::fmt;

pub use logger::Logger;
pub use progress::pause_rendering;

use thiserror::Error;

use crate::prelude::*;
use progress::DropHandle;

mod logger;
mod progress;

#[throws(Error)]
pub fn init() {
    Logger::init()?;
    progress::init();
}

#[throws(Error)]
pub fn cleanup() {
    progress::cleanup()?;
    Logger::cleanup();
}

pub fn progress(message: String) -> DropHandle {
    progress::get_progress().push_progress_log(message)
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("Unknown log level '{0}'")]
    UnknownLevel(String),

    #[error("render thread pause lock already acquired")]
    PauseLockAlreadyAcquired,

    #[error("Failed to set logger: {0}")]
    SetLogger(#[from] log_facade::SetLoggerError),

    #[error(transparent)]
    Format(#[from] fmt::Error),

    #[error(transparent)]
    Crossterm(#[from] crossterm::ErrorKind),
}
