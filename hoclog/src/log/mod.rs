use std::{
    fmt,
    sync::{Arc, Mutex},
};

use console::Style;
use dialoguer::{theme::Theme, Confirm, Input, Password, Select};
use thiserror::Error;

use crate::{context::PrintContext, prefix::PrefixPrefs, Never, Result};
pub use status::Status;
pub use stream::Stream;

mod status;
mod stream;

const INFO_FLAG: &str = "~";
const ERROR_FLAG: &str = "⚠︎";

#[derive(Debug, Error)]
pub enum Error {
    #[error("a log error was printed")]
    ErrorLogged,

    #[error("the operation was aborted")]
    UserAborted,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum LogType {
    /// The start part of a nested log, such as status.
    NestedStart,

    /// The end part of a nested log, such as status.
    NestedEnd,

    /// All other logs ar flat.
    Flat,
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

    pub fn stream(&self) -> Stream {
        Stream::new(self)
    }

    pub fn status(&self, message: impl AsRef<str>) -> Status {
        let mut print_context = self.print_context.lock().unwrap();

        if print_context.status_level() == 0 {
            print_context.failure = false;
        }

        print_context.increment_status();
        print_context.decorated_println(
            message,
            LogType::NestedStart,
            PrefixPrefs::with_connector("╓╴").flag("*"),
            PrefixPrefs::in_status_overflow(),
        );

        Status::new(Arc::clone(&self.print_context))
    }

    pub fn info(&self, message: impl AsRef<str>) {
        let mut print_context = self.print_context.lock().unwrap();

        print_context.decorated_println(
            message,
            LogType::Flat,
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
            LogType::Flat,
            PrefixPrefs::in_status().flag(INFO_FLAG).label(&label),
            PrefixPrefs::in_status_overflow().label(&" ".repeat(1 + label_len)),
        );
    }

    pub fn warning(&self, message: impl AsRef<str>) -> Result<()> {
        let mut print_context = self.print_context.lock().unwrap();

        let yellow = Style::new().yellow();

        print_context.decorated_println(
            yellow.apply_to(message.as_ref()).to_string(),
            LogType::Flat,
            PrefixPrefs::in_status().flag(&yellow.apply_to(ERROR_FLAG).to_string()),
            PrefixPrefs::in_status_overflow(),
        );

        if self.prompt_impl(&mut print_context, "Do you want to continue?") {
            Ok(())
        } else {
            print_context.failure = true;
            Err(Error::UserAborted)
        }
    }

    pub fn error(&self, message: impl AsRef<str>) -> Result<Never> {
        let mut print_context = self.print_context.lock().unwrap();

        let red = Style::new().red();
        print_context.failure = true;

        print_context.decorated_println(
            red.apply_to(message.as_ref()).to_string(),
            LogType::Flat,
            PrefixPrefs::in_status().flag(&red.apply_to(ERROR_FLAG).to_string()),
            PrefixPrefs::in_status_overflow(),
        );

        Err(Error::ErrorLogged)
    }

    pub fn prompt(&self, message: impl AsRef<str>) -> bool {
        self.prompt_impl(&mut self.print_context.lock().unwrap(), message)
    }

    fn prompt_impl(&self, print_context: &mut PrintContext, message: impl AsRef<str>) -> bool {
        let cyan = Style::new().cyan();

        let mut prompt = print_context.create_line_prefix(PrefixPrefs::in_status().flag("?"));
        prompt += message.as_ref();

        let want_continue = Confirm::new()
            .with_prompt(cyan.apply_to(prompt).to_string())
            .default(false)
            .interact_on(&print_context.stdout)
            .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

        print_context.set_last_log_type(LogType::Flat);

        want_continue
    }

    pub fn input(&self, message: impl AsRef<str>) -> String {
        let mut print_context = self.print_context.lock().unwrap();

        let cyan = Style::new().cyan();

        let mut prompt = print_context.create_line_prefix(PrefixPrefs::in_status().flag("?"));
        prompt += message.as_ref();

        let input = Input::new()
            .with_prompt(cyan.apply_to(prompt).to_string())
            .interact_on(&print_context.stdout)
            .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

        print_context.set_last_log_type(LogType::Flat);

        input
    }

    pub fn hidden_input(&self, message: impl AsRef<str>) -> String {
        let mut print_context = self.print_context.lock().unwrap();

        let cyan = Style::new().cyan();

        let mut prompt = print_context.create_line_prefix(PrefixPrefs::in_status().flag("?"));
        prompt += message.as_ref();

        let password = Password::new()
            .with_prompt(cyan.apply_to(prompt).to_string())
            .interact_on(&print_context.stdout)
            .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

        print_context.set_last_log_type(LogType::Flat);

        password
    }

    pub fn choose(
        &self,
        message: impl AsRef<str>,
        items: impl IntoIterator<Item = impl ToString>,
        default_index: usize,
    ) -> usize {
        let mut print_context = self.print_context.lock().unwrap();

        let cyan = Style::new().cyan();
        let items: Vec<_> = items.into_iter().collect();

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

        let index = Select::with_theme(&ChooseTheme {
            print_context: &print_context,
        })
        .with_prompt(cyan.apply_to(prompt).to_string())
        .items(&items)
        .default(default_index)
        .interact_on_opt(&print_context.stdout)
        .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

        print_context.set_last_log_type(LogType::Flat);

        if let Some(index) = index {
            index
        } else {
            // anyhow::bail!("User cancelled operation");
            panic!("User cancelled operation");
        }
    }
}
