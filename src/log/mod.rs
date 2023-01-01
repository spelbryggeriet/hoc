use std::fmt;

pub use logger::Logger;
pub use progress::{pause_rendering, ProgressHandle};

use chrono::Utc;
use crossterm::style::{Color, SetForegroundColor};
use log_facade::log_enabled;
use thiserror::Error;

use crate::prelude::*;

use self::logger::{LoggerBuffer, LoggerMeta};

mod logger;
mod progress;

pub const CLEAR_COLOR: SetForegroundColor = SetForegroundColor(Color::Reset);
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

pub fn progress(message: String, level: Option<Level>, module: &'static str) -> ProgressHandle {
    LoggerBuffer::get_or_init()
        .push(
            LoggerMeta {
                timestamp: Utc::now(),
                level: level.unwrap_or(Level::Info),
                module: Some(module.into()),
            },
            format!("[PROGRESS START] {message}"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    if level.is_none() || level.filter(|l| log_enabled!(*l)).is_some() {
        progress::Progress::get_or_init().push_progress_log(message, level, module)
    } else {
        ProgressHandle::new_for_buffer(message, level, module)
    }
}

pub fn level_color(level: Level) -> SetForegroundColor {
    match level {
        Level::Trace => TRACE_COLOR,
        Level::Debug => DEBUG_COLOR,
        Level::Info => INFO_COLOR,
        Level::Warn => WARN_COLOR,
        Level::Error => ERROR_COLOR,
    }
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
