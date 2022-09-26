use std::{
    borrow::Cow,
    collections::VecDeque,
    env,
    fs::{self, File},
    io::Write,
    sync::Mutex,
};

use chrono::{DateTime, Datelike, Utc};
use crossterm::{
    execute,
    style::{Color, Print, SetForegroundColor},
};
use log::{Level, LevelFilter, Log, Metadata, Record};
use thiserror::Error;

use crate::prelude::*;

const MAX_DEFAULT_LEVEL: Level = if cfg!(debug_assertions) {
    Level::Trace
} else {
    Level::Info
};

pub struct Logger {
    level: Level,
    start_time: DateTime<Utc>,
    payload: Mutex<Payload>,
}

struct Payload {
    buffer: VecDeque<(DateTime<Utc>, Level, Option<String>, String)>,
    longest_mod_name: usize,
}

impl Logger {
    #[throws(Error)]
    pub fn init() {
        let level_str = env::var("RUST_LOG")
            .map(|v| Cow::Owned(v.to_uppercase()))
            .unwrap_or(Cow::Borrowed(MAX_DEFAULT_LEVEL.as_str()));
        let level = match &*level_str {
            "E" | "ER" | "ERR" | "ERRO" | "ERROU" => Level::Error,
            "W" | "WA" | "WAR" | "WARN" | "WARNI" | "WARNIN" | "WARNING" => Level::Warn,
            "I" | "IN" | "INF" | "INFO" => Level::Info,
            "D" | "DE" | "DEB" | "DEBU" | "DEBUG" => Level::Debug,
            "T" | "TR" | "TRA" | "TRAC" | "TRACE" => Level::Trace,
            _ => throw!(Error::UnknownLevel(level_str.to_string())),
        };

        let logger = Self {
            level,
            start_time: Utc::now(),
            payload: Mutex::new(Payload {
                buffer: VecDeque::new(),
                longest_mod_name: 0,
            }),
        };

        log::set_boxed_logger(Box::new(logger))?;
        log::set_max_level(LevelFilter::Trace);
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        let args_str = record.args().to_string();

        if self.enabled(record.metadata()) {
            let (level_icon, color) = match record.level() {
                Level::Error => ("\u{f00d}", Color::Red),
                Level::Warn => ("\u{f12a}", Color::Yellow),
                Level::Info => ("\u{fcaf}", Color::White),
                Level::Debug => ("\u{fd2b}", Color::DarkMagenta),
                Level::Trace => ("\u{e241}", Color::DarkGrey),
            };

            let mut log_line_bytes = Vec::new();
            execute!(
                log_line_bytes,
                SetForegroundColor(color),
                Print(format!("{level_icon} {args_str}")),
                SetForegroundColor(Color::Reset),
            )
            .expect("writing to `Vec` should always be successful");

            let log_line =
                String::from_utf8(log_line_bytes).expect("control sequence should be valid");
            println!("{log_line}");
        }

        {
            let mut payload = self.payload.lock().expect("thread should not be poisoned");
            payload.buffer.push_back((
                Utc::now(),
                record.level(),
                record.module_path().map(str::to_string),
                args_str,
            ));

            if payload.buffer.len() > 100 {
                drop(payload);
                self.flush();
            }
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
            .open(format!("{log_dir}/{:?}.txt", self.start_time))
            .expect("file should be unique");

        let mut error = None;
        {
            let mut payload = self.payload.lock().expect("thread should not be poisoned");
            let mut longest_mod_name = payload.longest_mod_name.max(
                payload
                    .buffer
                    .iter()
                    .map(|(_, _, module, _)| module.as_ref().map(|m| m.len()).unwrap_or(0))
                    .max()
                    .unwrap_or(0),
            );

            for (time, level, module, message) in payload.buffer.drain(..) {
                let res = if let Some(module) = module {
                    if module.len() > longest_mod_name {
                        longest_mod_name = module.len();
                    }

                    file.write_fmt(format_args!(
                        "[{time:?} {level:<7} {module:<longest_mod_name$}] {message}\n",
                    ))
                } else {
                    file.write_fmt(format_args!(
                        "[{time:?} {level:<7}{:mod_len$}] {message}\n",
                        "",
                        mod_len = if longest_mod_name > 0 {
                            longest_mod_name + 1
                        } else {
                            0
                        },
                    ))
                };

                if let Err(err) = res {
                    error.replace(err);
                    break;
                }
            }

            payload.longest_mod_name = longest_mod_name;
        }

        if let Some(err) = error {
            panic!("{err}");
        }
    }
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("Unknown log level '{0}'")]
    UnknownLevel(String),

    #[error("Failed to set logger: {0}")]
    SetLogger(#[from] log::SetLoggerError),
}
