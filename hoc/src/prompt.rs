use std::{
    borrow::Cow,
    fmt::{self, Debug, Display, Formatter},
    marker::PhantomData,
    str::FromStr,
};

use crossterm::{cursor, terminal};
use inquire::{
    error::CustomUserError,
    ui::{Color, RenderConfig, StyleSheet},
    validator::{ErrorMessage, Validation},
    Password, PasswordDisplayMode, Select, Text,
};
use thiserror::Error;

use crate::{log, prelude::*, util::Secret};

fn clear_prompt(lines: u16) -> String {
    use std::fmt::Write;

    let mut clear_section = String::new();
    write!(
        clear_section,
        "{move_column}{clear_line}{move_line}",
        clear_line = terminal::Clear(terminal::ClearType::CurrentLine),
        move_column = cursor::MoveToColumn(0),
        move_line = cursor::MoveToPreviousLine(lines),
    )
    .expect("commands should be formatable");

    clear_section
}

pub struct PromptBuilder<'a, T, S> {
    message: Cow<'static, str>,
    default: Option<Cow<'static, str>>,
    initial_input: Option<&'a str>,
    _output_type: PhantomData<T>,
    _state: PhantomData<S>,
}

pub trait NonSecret {}

pub enum Normal {}
impl NonSecret for Normal {}

pub enum WithDefault {}
impl NonSecret for WithDefault {}

pub enum AsSecret {}

impl<'a, T, S> PromptBuilder<'a, T, S> {
    fn convert<U>(self) -> PromptBuilder<'a, T, U> {
        PromptBuilder {
            message: self.message,
            default: self.default,
            initial_input: self.initial_input,
            _output_type: self._output_type,
            _state: Default::default(),
        }
    }
}
impl<'a, T> PromptBuilder<'a, T, Normal> {
    pub fn new(message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            message: message.into(),
            default: None,
            initial_input: None,
            _output_type: Default::default(),
            _state: Default::default(),
        }
    }

    pub fn with_default(
        mut self,
        default: impl Into<Cow<'static, str>>,
    ) -> PromptBuilder<'a, T, WithDefault> {
        self.default.replace(default.into());
        self.convert()
    }

    pub fn with_initial_input(
        mut self,
        initial_input: &'a str,
    ) -> PromptBuilder<'a, T, WithDefault> {
        self.initial_input.replace(initial_input);
        self.convert()
    }

    pub fn as_secret(self) -> PromptBuilder<'a, T, AsSecret> {
        self.convert()
    }
}

impl<'a, T, S> PromptBuilder<'a, T, S>
where
    T: FromStr,
    T::Err: Display,
    S: NonSecret,
{
    #[throws(Error)]
    pub fn get(self) -> T {
        let prompt = format!("{}:", self.message);

        let _pause_lock = log::pause_rendering()?;

        let default_clone = self.default.clone();
        let validator =
            move |s: &str| {
                let s = s.trim();
                if s.is_empty() {
                    return if let Some(default) = &default_clone {
                        match T::from_str(default) {
                            Ok(_) => Ok(Validation::Valid),
                            Err(_) => Err(Box::new(InvalidDefaultError(default.to_string()))
                                as CustomUserError),
                        }
                    } else {
                        Ok(Validation::Invalid(ErrorMessage::Custom(
                            "input must not be empty".to_string(),
                        )))
                    };
                }

                match T::from_str(s) {
                    Ok(_) => Ok(Validation::Valid),
                    Err(err) => Ok(Validation::Invalid(ErrorMessage::Custom(err.to_string()))),
                }
            };

        let mut text = Text::new(&prompt)
            .with_validator(validator)
            .with_formatter(&|_| clear_prompt(2));

        if let Some(default) = &self.default {
            text = text.with_default(default);
        }

        if let Some(initial_input) = self.initial_input {
            text = text.with_initial_value(initial_input);
        }

        match text.prompt() {
            Ok(resp) => T::from_str(resp.trim()).unwrap_or_else(|_| unreachable!()),
            Err(
                err @ (inquire::InquireError::Custom(_)
                | inquire::InquireError::OperationCanceled
                | inquire::InquireError::OperationInterrupted),
            ) => throw!(err),
            Err(err) => {
                if let Some(default) = &self.default {
                    T::from_str(default).unwrap_or_else(|_| unreachable!())
                } else {
                    throw!(err);
                }
            }
        }
    }
}

impl<'a, T> PromptBuilder<'a, T, AsSecret>
where
    T: FromStr,
    T::Err: Display,
{
    #[throws(Error)]
    pub fn get(self) -> Secret<T> {
        let prompt = format!("{}:", self.message);
        let prompt_confirm = format!("{} (confirm):", self.message);
        let _pause_lock = log::pause_rendering()?;

        let validator = move |s: &str| {
            let s = s.trim();
            if s.is_empty() {
                return Ok(Validation::Invalid(ErrorMessage::Custom(
                    "input must not be empty".to_string(),
                )));
            }

            match T::from_str(s) {
                Ok(_) => Ok(Validation::Valid),
                Err(err) => Ok(Validation::Invalid(ErrorMessage::Custom(err.to_string()))),
            }
        };

        let render_config = RenderConfig::default()
            .with_help_message(StyleSheet::default().with_fg(Color::DarkBlue));

        let text = Password::new(&prompt)
            .with_render_config(render_config)
            .with_custom_confirmation_message(&prompt_confirm)
            .with_display_mode(PasswordDisplayMode::Masked)
            .with_display_toggle_enabled()
            .with_help_message("Ctrl-R to reveal/hide")
            .with_validator(validator)
            .with_formatter(&|_| clear_prompt(2));

        let resp = text.prompt()?;
        Secret::new(T::from_str(resp.trim()).unwrap_or_else(|_| unreachable!()))
    }
}

pub struct SelectBuilder<'a, T> {
    message: Cow<'static, str>,
    options: Vec<SelectBuilderOption<'a, T>>,
}

impl<'a, T> SelectBuilder<'a, T> {
    pub fn new(message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            message: message.into(),
            options: Vec::with_capacity(1),
        }
        .with_abort_option()
    }

    fn with_abort_option(mut self) -> Self {
        self.options.push(SelectBuilderOption {
            title: "Abort",
            on_select: None,
        });
        self
    }

    pub fn with_option(mut self, title: &'static str, on_select: impl FnOnce() -> T + 'a) -> Self {
        self.options.push(SelectBuilderOption {
            title,
            on_select: Some(Box::new(on_select)),
        });
        self
    }

    #[throws(Error)]
    pub fn get(self) -> T {
        let _pause_lock = log::pause_rendering()?;

        let option = Select::new(&self.message, self.options)
            .with_formatter(&|_| clear_prompt(2))
            .prompt()?;

        info!("{} {}", self.message, option.title);

        option
            .on_select
            .map(|f| f())
            .ok_or(Error::Inquire(inquire::InquireError::OperationCanceled))?
    }
}

struct SelectBuilderOption<'a, T> {
    title: &'static str,
    on_select: Option<Box<dyn FnOnce() -> T + 'a>>,
}

impl<T> Display for SelectBuilderOption<'_, T> {
    #[throws(fmt::Error)]
    fn fmt(&self, f: &mut Formatter) {
        write!(f, "{}", self.title)?;
    }
}

#[derive(Debug, Error)]
#[error("Invalid default value: {0}")]
pub struct InvalidDefaultError(String);

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Log(#[from] log::Error),

    #[error(transparent)]
    Inquire(#[from] inquire::InquireError),
}

impl<T> private::Sealed for Option<T> {}

mod private {
    pub trait Sealed {}
}
