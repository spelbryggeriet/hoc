use std::{
    borrow::Cow,
    error::Error as StdError,
    io,
    result::Result as StdResult,
    sync::{Arc, Mutex},
};

use console::Style;
use dialoguer::Input;
use thiserror::Error;

use crate::{context::PrintContext, prefix::PrefixPrefs, Never, Result, LOG};
pub use status::Status;
pub use stream::Stream;

use self::{choose::Choose, hidden_input::HiddenInput, prompt::Prompt};

mod choose;
mod hidden_input;
mod prompt;
mod status;
mod stream;

const INFO_FLAG: &str = "~";
const ERROR_FLAG: &str = "⚠︎";

#[derive(Debug, Error)]
pub enum Error {
    #[error("A log error was printed.")]
    ErrorLogged,

    #[error("The operation was aborted.")]
    UserAborted,
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        StdResult::<Never, _>::Err(err)
            .log_context("io")
            .unwrap_err()
    }
}

pub trait LogErr<T> {
    type Err: StdError;

    fn log_err(self) -> Result<T>;
    fn log_context<S: AsRef<str>>(self, msg: S) -> Result<T>;
    fn log_with_context<S: AsRef<str>, F: FnOnce(Self::Err) -> S>(self, f: F) -> Result<T>;
}

impl<T, E: StdError> LogErr<T> for StdResult<T, E> {
    type Err = E;

    fn log_err(self) -> Result<T> {
        self.map_err(|err| LOG.error(err.to_string()).unwrap_err())
    }

    fn log_context<S: AsRef<str>>(self, msg: S) -> Result<T> {
        self.map_err(|err| LOG.error(format!("{}: {}", msg.as_ref(), err)).unwrap_err())
    }

    fn log_with_context<S: AsRef<str>, F: FnOnce(<Self as LogErr<T>>::Err) -> S>(
        self,
        f: F,
    ) -> Result<T> {
        self.map_err(|err| LOG.error(f(err)).unwrap_err())
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum LogType {
    StatusStart,
    StatusEnd,
    Info,
    Warning,
    Error,
    Input,
    Choose,
    Prompt,
}

pub struct Log {
    print_context: Arc<Mutex<PrintContext>>,
}

impl Log {
    pub fn new() -> Self {
        Self {
            print_context: Arc::new(Mutex::new(PrintContext::new())),
        }
    }

    pub(crate) fn set_failure(&self) {
        self.print_context.lock().unwrap().failure = true;
    }

    pub fn create_line_prefix(&self, flag: impl AsRef<str>) -> String {
        let print_context = self.print_context.lock().unwrap();
        print_context.create_line_prefix(PrefixPrefs::in_status().flag(flag.as_ref()))
    }

    pub fn stream(&self) -> Stream {
        Stream::new(self)
    }

    pub fn status(&self, message: impl AsRef<str>) -> Status {
        let mut print_context = self.print_context.lock().unwrap();

        if print_context.status_level() == 0 {
            print_context.failure = false;
        }

        print_context.print_spacing_if_needed(LogType::StatusStart);
        print_context.increment_status();
        print_context.decorated_println(
            message,
            None,
            LogType::StatusStart,
            PrefixPrefs::with_connector("╓╴").flag("*"),
            PrefixPrefs::in_status_overflow(),
        );

        Status::new(Arc::clone(&self.print_context))
    }

    pub fn info(&self, message: impl AsRef<str>) {
        let mut print_context = self.print_context.lock().unwrap();

        print_context.decorated_println(
            message,
            None,
            LogType::Info,
            PrefixPrefs::in_status().flag(INFO_FLAG),
            PrefixPrefs::in_status_overflow(),
        );
    }

    pub fn labelled_info(&self, label: impl AsRef<str>, message: impl AsRef<str>) {
        let label_len = label.as_ref().chars().count();
        let label_trimmed = label.as_ref().trim_end().to_string();
        let label_trimmed_len = label_trimmed.chars().count();

        let mut label = label_trimmed;
        label += ":";
        label += &" ".repeat(label_len - label_trimmed_len);

        let mut print_context = self.print_context.lock().unwrap();

        print_context.decorated_println(
            message,
            None,
            LogType::Info,
            PrefixPrefs::in_status().flag(INFO_FLAG).label(&label),
            PrefixPrefs::in_status_overflow().label(&" ".repeat(1 + label_len)),
        );
    }

    pub fn warning(&self, message: impl AsRef<str>) -> Prompt {
        let mut print_context = self.print_context.lock().unwrap();

        let yellow = Style::new().yellow();
        let flag = yellow.apply_to(ERROR_FLAG).to_string();
        print_context.decorated_println(
            message,
            Some(yellow),
            LogType::Warning,
            PrefixPrefs::in_status().flag(&flag),
            PrefixPrefs::in_status_overflow(),
        );

        self.prompt("Do you want to continue?")
    }

    pub fn error(&self, message: impl AsRef<str>) -> Result<Never> {
        let mut print_context = self.print_context.lock().unwrap();

        print_context.failure = true;

        let red = Style::new().red();
        let flag = red.apply_to(ERROR_FLAG).to_string();
        print_context.decorated_println(
            message,
            Some(red),
            LogType::Error,
            PrefixPrefs::in_status().flag(&flag),
            PrefixPrefs::in_status_overflow(),
        );

        Err(Error::ErrorLogged)
    }

    pub fn prompt<'a, C: Into<Cow<'a, str>>>(&self, message: C) -> Prompt<'a> {
        Prompt::new(Arc::clone(&self.print_context), message.into())
    }

    pub fn input(&self, message: impl AsRef<str>) -> String {
        let mut print_context = self.print_context.lock().unwrap();

        print_context.print_spacing_if_needed(LogType::Input);

        let mut prompt = print_context.create_line_prefix(PrefixPrefs::in_status().flag(">"));
        prompt += message.as_ref();

        let cyan = Style::new().cyan();
        let input = Input::new()
            .with_prompt(cyan.apply_to(prompt).to_string())
            .interact_on(&print_context.stdout)
            .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

        input
    }

    pub fn hidden_input<'a, C: Into<Cow<'a, str>>>(&self, message: C) -> HiddenInput<'a> {
        HiddenInput::new(Arc::clone(&self.print_context), message.into())
    }

    pub fn choose<'a, T, C: Into<Cow<'a, str>>>(&self, message: C) -> Choose<'a, T> {
        Choose::new(Arc::clone(&self.print_context), message.into())
    }
}
