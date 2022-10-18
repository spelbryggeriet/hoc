use std::{
    borrow::Cow,
    env,
    fs::{self, File},
    io::Write,
    panic,
    sync::Mutex,
};

use chrono::{DateTime, Utc};
use log::{Level, LevelFilter, Log, Metadata, Record};
use thiserror::Error;

use crate::prelude::*;
pub use render::{pause, progress, Error as RenderError};

mod render;

const MAX_DEFAULT_LEVEL: Level = if cfg!(debug_assertions) {
    Level::Trace
} else {
    Level::Info
};

pub struct Logger {
    level: Level,
    start_time: DateTime<Utc>,
    buffer: Mutex<LoggerBuffer>,
}

impl Logger {
    #[throws(Error)]
    pub fn init() {
        let level_str = env::var("RUST_LOG")
            .map(|v| Cow::Owned(v.to_uppercase()))
            .unwrap_or(Cow::Borrowed(MAX_DEFAULT_LEVEL.as_str()));
        let level = match &*level_str {
            error if "ERROR".starts_with(error) => Level::Error,
            warning if "WARNING".starts_with(warning) => Level::Warn,
            info if "INFO".starts_with(info) => Level::Info,
            debug if "DEBUG".starts_with(debug) => Level::Debug,
            trace if "TRACE".starts_with(trace) => Level::Trace,
            _ => throw!(Error::UnknownLevel(level_str.to_string())),
        };

        let logger = Self {
            level,
            start_time: Utc::now(),
            buffer: Mutex::new(LoggerBuffer::new()),
        };

        log::set_boxed_logger(Box::new(logger))?;
        log::set_max_level(LevelFilter::Trace);

        render::RENDER_THREAD
            .set(render::RenderThread::init())
            .unwrap_or_else(|_| panic!("render thread already initialized"));
    }

    #[throws(Error)]
    pub fn cleanup() {
        log::logger().flush();
        render::RENDER_THREAD
            .get()
            .expect(EXPECT_RENDER_THREAD_INITIALIZED)
            .terminate()?;
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        let args_str = record.args().to_string();

        if self.enabled(record.metadata()) {
            render::RENDER_THREAD
                .get()
                .expect(EXPECT_RENDER_THREAD_INITIALIZED)
                .push_simple_log(record.level(), args_str.clone());
        }

        let mut buffer_lock = self.buffer.lock().unwrap_or_else(|err| panic!("{err}"));
        buffer_lock.messages.push((
            LoggerMeta {
                timestamp: Utc::now(),
                level: record.level(),
                module: record.module_path().map(str::to_string),
            },
            args_str,
        ));

        if buffer_lock.messages.len() >= 100 {
            drop(buffer_lock);
            self.flush();
        }
    }

    fn flush(&self) {
        let home_dir = env::var("HOME").expect("HOME environment variable should exist");
        let log_dir = format!(
            "{home_dir}/.local/share/hoc/logs/{}",
            self.start_time.format("%Y/%m/%d"),
        );
        fs::create_dir_all(&log_dir).expect("directories should be able to be created");
        let mut file = File::options()
            .create(true)
            .append(true)
            .open(format!(
                "{log_dir}/{}.txt",
                self.start_time.format("%T.%6f")
            ))
            .expect("file should be unique");

        {
            let mut buffer_lock = self.buffer.lock().unwrap_or_else(|err| panic!("{err}"));
            let mut longest_mod_name = buffer_lock.longest_mod_name.max(
                buffer_lock
                    .messages
                    .iter()
                    .filter_map(|(meta, _)| meta.module.as_ref().map(String::len))
                    .max()
                    .unwrap_or(0),
            );

            for (meta, message) in buffer_lock.messages.drain(..) {
                let res = if let Some(module) = &meta.module {
                    if module.len() > longest_mod_name {
                        longest_mod_name = module.len();
                    }

                    write!(
                        file,
                        "[{time:<27} {level:<7} {module:<longest_mod_name$}] {message}\n",
                        level = meta.level,
                        time = format!("{:?}", meta.timestamp),
                    )
                } else {
                    write!(
                        file,
                        "[{time:<27} {level:<7}{empty_mod:mod_len$}] {message}\n",
                        empty_mod = "",
                        level = meta.level,
                        mod_len = if longest_mod_name > 0 {
                            longest_mod_name + 1
                        } else {
                            0
                        },
                        time = format!("{:?}", meta.timestamp),
                    )
                };

                if let Err(err) = res {
                    panic!("{err}");
                }
            }

            buffer_lock.longest_mod_name = longest_mod_name;
        }
    }
}

struct LoggerBuffer {
    messages: Vec<(LoggerMeta, String)>,
    longest_mod_name: usize,
}

impl LoggerBuffer {
    const fn new() -> Self {
        Self {
            messages: Vec::new(),
            longest_mod_name: 0,
        }
    }
}

struct LoggerMeta {
    timestamp: DateTime<Utc>,
    level: Level,
    module: Option<String>,
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("Unknown log level '{0}'")]
    UnknownLevel(String),

    #[error("Failed to set logger: {0}")]
    SetLogger(#[from] log::SetLoggerError),

    #[error(transparent)]
    Crossterm(#[from] crossterm::ErrorKind),

    #[error(transparent)]
    Render(#[from] render::Error),
}
