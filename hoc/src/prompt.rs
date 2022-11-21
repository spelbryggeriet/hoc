use std::{
    borrow::Cow,
    fmt::{Debug, Display},
    marker::PhantomData,
    str::FromStr,
};

use inquire::{
    error::CustomUserError,
    ui::{Color, RenderConfig, StyleSheet},
    validator::{ErrorMessage, Validation},
    Password, PasswordDisplayMode, Select, Text,
};
use thiserror::Error;

use crate::{log, prelude::*};

fn postpad(lines: u16) {
    for _ in 0..lines {
        println!();
    }
}

pub struct PromptBuilder<'a, T, S> {
    message: Cow<'static, str>,
    default: Option<Cow<'static, str>>,
    initial_input: Option<&'a str>,
    help_message: Option<&'a str>,
    _output_type: PhantomData<T>,
    _state: PhantomData<S>,
}

pub enum NonSecret {}
pub enum AsSecret {}

impl<'a, T> PromptBuilder<'a, T, NonSecret> {
    pub fn new(message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            message: message.into(),
            default: None,
            initial_input: None,
            help_message: None,
            _output_type: Default::default(),
            _state: Default::default(),
        }
    }

    pub fn as_secret(self) -> PromptBuilder<'a, T, AsSecret> {
        PromptBuilder {
            message: self.message,
            default: self.default,
            initial_input: self.initial_input,
            help_message: self.help_message,
            _output_type: self._output_type,
            _state: Default::default(),
        }
    }
}

impl<'a, T, S> PromptBuilder<'a, T, S> {
    pub fn with_default(mut self, default: impl Into<Cow<'static, str>>) -> Self {
        self.default.replace(default.into());
        self
    }

    pub fn with_initial_input(mut self, initial_input: &'a str) -> Self {
        self.initial_input.replace(initial_input);
        self
    }

    pub fn with_help_message(mut self, help_message: &'a str) -> Self {
        self.help_message.replace(help_message);
        self
    }
}

impl<'a, T> PromptBuilder<'a, T, NonSecret>
where
    T: FromStr,
    T::Err: Display,
{
    #[throws(Error)]
    pub fn get(self) -> T {
        let prompt = format!("{}:", self.message);

        let pause_height = 2 + self.help_message.map_or(0, |_| 1);
        let pause_lock = log::pause_rendering(pause_height)?;

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

        let render_config =
            RenderConfig::default().with_global_indentation(pause_lock.indentation() as u16);

        let mut text = Text::new(&prompt)
            .with_render_config(render_config)
            .with_validator(validator);

        if let Some(default) = &self.default {
            text = text.with_default(default);
        }

        if let Some(initial_input) = self.initial_input {
            text = text.with_initial_value(initial_input);
        }

        if let Some(help_message) = self.help_message {
            text = text.with_help_message(help_message);
        }

        let res = text.prompt();

        match res {
            Ok(resp) => {
                let resp_str = resp.trim();
                pause_lock
                    .finish_with_message(Level::Info, format!("{}: {resp_str}", self.message));

                T::from_str(resp_str).unwrap_or_else(|_| unreachable!())
            }
            Err(
                err @ (inquire::InquireError::Custom(_)
                | inquire::InquireError::OperationCanceled
                | inquire::InquireError::OperationInterrupted),
            ) => throw!(err),
            Err(err) => {
                let Some(default) = &self.default else {
                    throw!(err);
                };
                pause_lock.finish_with_message(Level::Info, format!("{}: {default}", self.message));

                T::from_str(default).unwrap_or_else(|_| unreachable!())
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
        let pause_lock = log::pause_rendering(4)?;

        let prompt = format!("{}:", self.message);
        let prompt_confirm = format!("{} (confirm):", self.message);
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
            .with_global_indentation(pause_lock.indentation() as u16)
            .with_help_message(StyleSheet::default().with_fg(Color::DarkBlue));

        let text = Password::new(&prompt)
            .with_render_config(render_config)
            .with_custom_confirmation_message(&prompt_confirm)
            .with_display_mode(PasswordDisplayMode::Masked)
            .with_display_toggle_enabled()
            .with_help_message("Ctrl-R to reveal/hide")
            .with_validator(validator);

        let res = text.prompt();
        postpad(2);

        let secret = Secret::new(T::from_str(res?.trim()).unwrap_or_else(|_| unreachable!()));
        pause_lock.finish_with_message(Level::Info, format!("{}: {secret}", self.message));

        secret
    }
}

pub struct SelectBuilder<T> {
    message: Cow<'static, str>,
    options: Vec<T>,
}

impl<T> SelectBuilder<T> {
    pub fn new(message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            message: message.into(),
            options: Vec::with_capacity(1),
        }
    }

    pub fn with_option(mut self, option: T) -> Self {
        self.options.push(option);
        self
    }

    pub fn with_options<I: IntoIterator<Item = T>>(mut self, options: I) -> Self {
        self.options.extend(options);
        self
    }

    pub fn option_count(&self) -> usize {
        self.options.len()
    }
}

impl<T: Display> SelectBuilder<T> {
    #[throws(Error)]
    pub fn get(self) -> T {
        let num_options = self.options.len();
        let pause_lock = log::pause_rendering(2 + num_options)?;

        let render_config =
            RenderConfig::default().with_global_indentation(pause_lock.indentation() as u16);

        let option = Select::new(&self.message, self.options)
            .with_render_config(render_config)
            .prompt();
        postpad(num_options as u16);

        let option = option?;
        pause_lock.finish_with_message(Level::Info, format!("{} {}", self.message, option));

        option
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
