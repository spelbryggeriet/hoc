use std::{
    borrow::Cow,
    fmt::{Debug, Display},
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

pub struct PromptBuilder<T, S> {
    message: Cow<'static, str>,
    default: Option<Cow<'static, str>>,
    _output_type: PhantomData<T>,
    _state: PhantomData<S>,
}

pub trait NonSecret {}

pub enum Normal {}
impl NonSecret for Normal {}

pub enum WithDefault {}
impl NonSecret for WithDefault {}

pub enum AsSecret {}

impl<T, S> PromptBuilder<T, S> {
    fn convert<U>(self) -> PromptBuilder<T, U> {
        PromptBuilder {
            message: self.message,
            default: self.default,
            _output_type: self._output_type,
            _state: Default::default(),
        }
    }
}
impl<T> PromptBuilder<T, Normal> {
    pub fn new(message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            message: message.into(),
            default: None,
            _output_type: Default::default(),
            _state: Default::default(),
        }
    }

    pub fn with_default(
        mut self,
        default: impl Into<Cow<'static, str>>,
    ) -> PromptBuilder<T, WithDefault> {
        self.default.replace(default.into());
        self.convert()
    }

    pub fn as_secret(self) -> PromptBuilder<T, AsSecret> {
        self.convert()
    }
}

impl<T, S> PromptBuilder<T, S>
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

        if let Some(ref default) = self.default {
            text = text.with_default(default);
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

impl<T> PromptBuilder<T, AsSecret>
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

pub struct SelectBuilder<T, S> {
    message: Cow<'static, str>,
    options: Vec<T>,
    _state: PhantomData<S>,
}

pub enum Empty {}
pub enum NonEmpty {}

impl<T> SelectBuilder<T, Empty> {
    pub fn new(message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            message: message.into(),
            options: Vec::with_capacity(1),
            _state: Default::default(),
        }
    }
}

impl<T, S> SelectBuilder<T, S> {
    fn into_non_empty(self) -> SelectBuilder<T, NonEmpty> {
        SelectBuilder {
            message: self.message,
            options: self.options,
            _state: Default::default(),
        }
    }

    pub fn with_option(mut self, option: T) -> SelectBuilder<T, NonEmpty> {
        self.options.push(option);
        self.into_non_empty()
    }

    pub fn with_options(
        mut self,
        options: impl IntoIterator<Item = T>,
    ) -> SelectBuilder<T, NonEmpty> {
        self.options.extend(options);
        self.into_non_empty()
    }
}

impl<T: Display + Send + 'static> SelectBuilder<T, NonEmpty> {
    #[throws(Error)]
    pub fn get(self) -> T {
        let _pause_lock = log::pause_rendering()?;

        Select::new(&self.message, self.options)
            .with_formatter(&|_| clear_prompt(2))
            .prompt()?
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
