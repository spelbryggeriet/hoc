use std::fmt;

use chrono::Utc;
use crossterm::style::{Color, SetForegroundColor};
pub use logger::Logger;
pub use progress::pause_rendering;

use thiserror::Error;

use crate::prelude::*;
use progress::DropHandle;

use self::logger::{LoggerBuffer, LoggerMeta};

mod logger;
mod progress;

pub const ERROR_COLOR: SetForegroundColor = SetForegroundColor(Color::Red);
pub const WARN_COLOR: SetForegroundColor = SetForegroundColor(Color::Yellow);
pub const INFO_COLOR: SetForegroundColor = SetForegroundColor(Color::White);
pub const DEBUG_COLOR: SetForegroundColor = SetForegroundColor(Color::DarkMagenta);
pub const TRACE_COLOR: SetForegroundColor = SetForegroundColor(Color::DarkGrey);

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
