use std::fmt;

use chrono::Utc;
pub use logger::Logger;
pub use progress::pause_rendering;

use thiserror::Error;

use crate::prelude::*;
use progress::DropHandle;

use self::logger::{LoggerBuffer, LoggerMeta};

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

pub fn progress(message: String, module: &'static str) -> DropHandle {
    LoggerBuffer::get_or_init()
        .push(
            LoggerMeta {
                timestamp: Utc::now(),
                level: Level::Info,
                module: Some(module.into()),
            },
            format!("[PROGRESS START] {message}"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    progress::Progress::get_or_init().push_progress_log(message, module)
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
