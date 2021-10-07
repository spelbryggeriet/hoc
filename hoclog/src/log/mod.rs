use std::{
    error::Error as StdError,
    fmt,
    result::Result as StdResult,
    sync::{Arc, Mutex},
};

use console::Style;
use dialoguer::{theme::Theme, Confirm, Input, Password, Select};
use thiserror::Error;

use crate::{context::PrintContext, prefix::PrefixPrefs, Never, Result, LOG};
pub use status::Status;
pub use stream::Stream;

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

    #[error("An empty list of items was sent to `choose`.")]
    ChooseNoItems,
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
        print_context.create_line_prefix(PrefixPrefs::in_status_overflow().flag(flag.as_ref()))
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
            LogType::StatusStart,
            PrefixPrefs::with_connector("╓╴").flag("*"),
            PrefixPrefs::in_status_overflow(),
        );

        Status::new(Arc::clone(&self.print_context))
    }

    pub fn info(&self, message: impl AsRef<str>) {
        let mut print_context = self.print_context.lock().unwrap();

        for line in message.as_ref().lines() {
            print_context.decorated_println(
                line,
                LogType::Info,
                PrefixPrefs::in_status().flag(INFO_FLAG),
                PrefixPrefs::in_status_overflow(),
            );
        }
    }

    pub fn labelled_info(&self, label: impl AsRef<str>, message: impl AsRef<str>) {
        let label_len = label.as_ref().chars().count();
        let label_trimmed = label.as_ref().trim_end().to_string();
        let label_trimmed_len = label_trimmed.chars().count();

        let mut label = label_trimmed;
        label += ":";
        label += &" ".repeat(label_len - label_trimmed_len);

        let mut print_context = self.print_context.lock().unwrap();

        for line in message.as_ref().lines() {
            print_context.decorated_println(
                line,
                LogType::Info,
                PrefixPrefs::in_status().flag(INFO_FLAG).label(&label),
                PrefixPrefs::in_status_overflow().label(&" ".repeat(1 + label_len)),
            );
        }
    }

    pub fn warning(&self, message: impl AsRef<str>) -> Result<()> {
        let mut print_context = self.print_context.lock().unwrap();

        let yellow = Style::new().yellow();
        for line in message.as_ref().lines() {
            print_context.decorated_println(
                yellow.apply_to(line).to_string(),
                LogType::Warning,
                PrefixPrefs::in_status().flag(&yellow.apply_to(ERROR_FLAG).to_string()),
                PrefixPrefs::in_status_overflow(),
            );
        }

        self.prompt_impl(&mut print_context, "Do you want to continue?")
    }

    pub fn error(&self, message: impl AsRef<str>) -> Result<Never> {
        let mut print_context = self.print_context.lock().unwrap();

        print_context.failure = true;

        let red = Style::new().red();
        for line in message.as_ref().lines() {
            print_context.decorated_println(
                red.apply_to(line).to_string(),
                LogType::Error,
                PrefixPrefs::in_status().flag(&red.apply_to(ERROR_FLAG).to_string()),
                PrefixPrefs::in_status_overflow(),
            );
        }

        Err(Error::ErrorLogged)
    }

    pub fn prompt(&self, message: impl AsRef<str>) -> Result<()> {
        self.prompt_impl(&mut self.print_context.lock().unwrap(), message)
    }

    fn prompt_impl(
        &self,
        print_context: &mut PrintContext,
        message: impl AsRef<str>,
    ) -> Result<()> {
        print_context.print_spacing_if_needed(LogType::Prompt);

        let mut prompt = print_context.create_line_prefix(PrefixPrefs::in_status().flag("?"));
        prompt += message.as_ref();

        let cyan = Style::new().cyan();
        let want_continue = Confirm::new()
            .with_prompt(cyan.apply_to(prompt).to_string())
            .default(false)
            .interact_on(&print_context.stdout)
            .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

        if want_continue {
            Ok(())
        } else {
            print_context.failure = true;
            Err(Error::UserAborted)
        }
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

    pub fn hidden_input(&self, message: impl AsRef<str>) -> String {
        let mut print_context = self.print_context.lock().unwrap();

        print_context.print_spacing_if_needed(LogType::Input);

        let mut prompt = print_context.create_line_prefix(PrefixPrefs::in_status().flag(">"));
        prompt += message.as_ref();

        let cyan = Style::new().cyan();
        let password = Password::new()
            .with_prompt(cyan.apply_to(prompt).to_string())
            .interact_on(&print_context.stdout)
            .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

        password
    }

    pub fn choose(
        &self,
        message: impl AsRef<str>,
        items: impl IntoIterator<Item = impl ToString>,
        default_index: usize,
    ) -> Result<usize> {
        let items: Vec<_> = items.into_iter().collect();
        if items.len() == 0 {
            return Err(Error::ChooseNoItems);
        }

        let mut print_context = self.print_context.lock().unwrap();

        print_context.print_spacing_if_needed(LogType::Choose);

        let mut prompt = print_context.create_line_prefix(PrefixPrefs::in_status().flag("#"));
        prompt += message.as_ref();

        struct ChooseTheme<'a> {
            print_context: &'a PrintContext,
        }

        impl Theme for ChooseTheme<'_> {
            fn format_select_prompt_item(
                &self,
                f: &mut dyn fmt::Write,
                text: &str,
                active: bool,
            ) -> fmt::Result {
                let prefix = self.print_context.create_line_prefix(
                    PrefixPrefs::in_status_overflow().flag(if active { ">" } else { " " }),
                );
                write!(f, "{}{}", prefix, text)
            }
        }

        let cyan = Style::new().cyan();
        let index = Select::with_theme(&ChooseTheme {
            print_context: &print_context,
        })
        .with_prompt(cyan.apply_to(prompt).to_string())
        .items(&items)
        .default(default_index)
        .interact_on_opt(&print_context.stdout)
        .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

        if let Some(index) = index {
            Ok(index)
        } else {
            Err(Error::UserAborted)
        }
    }
}
