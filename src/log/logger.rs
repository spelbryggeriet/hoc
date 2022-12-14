use std::{
    borrow::Cow,
    env,
    fs::{self, File},
    io::Write,
    panic,
    sync::{Mutex, MutexGuard},
};

use chrono::{DateTime, Utc};
use log_facade::{Level, LevelFilter, Log, Metadata, Record};
use once_cell::sync::OnceCell;

use crate::{
    log::{self, progress::Progress, Error},
    prelude::*,
};

const MAX_DEFAULT_LEVEL: Level = if cfg!(debug_assertions) {
    Level::Trace
} else {
    Level::Info
};

static START_TIME: OnceCell<DateTime<Utc>> = OnceCell::new();

pub struct Logger {
    level: Level,
}

impl Logger {
    #[throws(Error)]
    pub(super) fn init() {
        let level_str = env::var("HOC_LOG")
            .map(|v| Cow::Owned(v.to_uppercase()))
            .unwrap_or(Cow::Borrowed(MAX_DEFAULT_LEVEL.as_str()));
        let level = match &*level_str {
            error if "ERROR".starts_with(error) => Level::Error,
            warning if "WARNING".starts_with(warning) => Level::Warn,
            info if "INFO".starts_with(info) => Level::Info,
            debug if "DEBUG".starts_with(debug) => Level::Debug,
            trace if "TRACE".starts_with(trace) => Level::Trace,
            _ => throw!(Error::UnknownLevel(level_str.into_owned())),
        };

        START_TIME.get_or_init(Utc::now);

        let logger = Self { level };

        log_facade::set_boxed_logger(Box::new(logger))?;
        log_facade::set_max_level(LevelFilter::Trace);
    }

    pub(super) fn cleanup() {
        log_facade::logger().flush();
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        let args_str = record.args().to_string();

        if self.enabled(record.metadata()) {
            Progress::get_or_init().push_simple_log(record.level(), args_str.clone());
        }

        LoggerBuffer::get_or_init()
            .push(
                LoggerMeta {
                    timestamp: Utc::now(),
                    level: record.level(),
                    module: record.module_path().map(|m| m.to_owned().into()),
                },
                args_str,
            )
            .unwrap_or_else(|e| panic!("{e}"));
    }

    fn flush(&self) {
        LoggerBuffer::get_or_init()
            .flush()
            .unwrap_or_else(|e| panic!("{e}"));
    }
}

pub(in crate::log) struct LoggerBuffer {
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

    pub fn get_or_init() -> MutexGuard<'static, Self> {
        static LOGGER_BUFFER: OnceCell<Mutex<LoggerBuffer>> = OnceCell::new();

        LOGGER_BUFFER
            .get_or_init(|| Mutex::new(Self::new()))
            .lock()
            .expect(EXPECT_THREAD_NOT_POSIONED)
    }

    #[throws(anyhow::Error)]
    pub fn push(&mut self, meta: LoggerMeta, args: String) {
        self.messages.push((meta, args));

        if self.messages.len() >= 100 {
            self.flush()?;
        }
    }

    #[throws(anyhow::Error)]
    pub fn flush(&mut self) {
        let start_time = START_TIME.get().expect("start time should be initialized");
        let home_dir = env::var("HOME").context("HOME environment variable should exist")?;
        let log_dir = format!(
            "{home_dir}/.local/share/hoc/logs/{}",
            start_time.format("%Y/%m/%d"),
        );
        fs::create_dir_all(&log_dir).context("directories should be able to be created")?;
        let mut file = File::options()
            .create(true)
            .append(true)
            .open(format!("{log_dir}/{}.txt", start_time.format("%T.%6f")))
            .context("file should be unique")?;

        let mut longest_mod_name = self.longest_mod_name.max(
            self.messages
                .iter()
                .filter_map(|(meta, _)| meta.module.as_deref().map(str::len))
                .max()
                .unwrap_or(0),
        );

        for (meta, message) in self.messages.drain(..) {
            let color = log::level_color(meta.level);

            let res = if let Some(module) = &meta.module {
                if module.len() > longest_mod_name {
                    longest_mod_name = module.len();
                }

                writeln!(
                    file,
                    "{color}[{time:<27} {level:<7} {module:<longest_mod_name$}] {message}{CLEAR_COLOR}",
                    level = meta.level,
                    time = format!("{:?}", meta.timestamp),
                )
            } else {
                writeln!(
                    file,
                    "{color}[{time:<27} {level:<7}{empty_mod:mod_len$}] {message}{CLEAR_COLOR}",
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

        self.longest_mod_name = longest_mod_name;
    }
}

pub(in crate::log) struct LoggerMeta {
    pub timestamp: DateTime<Utc>,
    pub level: Level,
    pub module: Option<Cow<'static, str>>,
}
