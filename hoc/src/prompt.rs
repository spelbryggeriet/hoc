use std::{
    borrow::Cow,
    fmt::{self, Debug, Display, Formatter},
    marker::PhantomData,
    str::FromStr,
};

use inquire::{
    error::CustomUserError,
    ui::{Color, RenderConfig, StyleSheet, Styled},
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

fn into_inquire_color(color: crossterm::style::Color) -> Color {
    use crossterm::style::Color as C;

    match color {
        C::Black => Color::Black,
        C::Red => Color::LightRed,
        C::DarkRed => Color::DarkRed,
        C::Green => Color::LightGreen,
        C::DarkGreen => Color::DarkGreen,
        C::Yellow => Color::LightYellow,
        C::DarkYellow => Color::DarkYellow,
        C::Blue => Color::LightBlue,
        C::DarkBlue => Color::DarkBlue,
        C::Magenta => Color::LightMagenta,
        C::DarkMagenta => Color::DarkMagenta,
        C::Cyan => Color::LightCyan,
        C::DarkCyan => Color::DarkCyan,
        C::White => Color::White,
        C::Grey => Color::Grey,
        C::DarkGrey => Color::DarkGrey,
        C::Rgb { r, g, b } => Color::Rgb { r, g, b },
        C::AnsiValue(b) => Color::AnsiValue(b),
        C::Reset => panic!("`Reset` is not an inquire color"),
    }
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

        let pause_lock = log::pause_rendering(2)?;

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

        let (color, prefix) = pause_lock.prefix();
        let render_config = RenderConfig::default()
            .with_global_prefix(Styled::new(prefix).with_fg(into_inquire_color(color)));

        let mut text = Text::new(&prompt)
            .with_render_config(render_config)
            .with_validator(validator);

        if let Some(default) = &self.default {
            text = text.with_default(default);
        }

        if let Some(initial_input) = self.initial_input {
            text = text.with_initial_value(initial_input);
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

        let (color, prefix) = pause_lock.prefix();
        let render_config = RenderConfig::default()
            .with_global_prefix(Styled::new(prefix).with_fg(into_inquire_color(color)))
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
    }

    pub fn with_option(mut self, title: &'static str, on_select: impl FnOnce() -> T + 'a) -> Self {
        self.options.push(SelectBuilderOption {
            title,
            on_select: Box::new(on_select),
        });
        self
    }

    #[throws(Error)]
    pub fn get(self) -> T {
        let num_options = self.options.len();
        let pause_lock = log::pause_rendering(2 + num_options)?;

        let (color, prefix) = pause_lock.prefix();
        let render_config = RenderConfig::default()
            .with_global_prefix(Styled::new(prefix).with_fg(into_inquire_color(color)));

        let option = Select::new(&self.message, self.options)
            .with_render_config(render_config)
            .prompt();
        postpad(num_options as u16);

        let option = option?;
        pause_lock.finish_with_message(Level::Info, format!("{} {}", self.message, option.title));

        (option.on_select)()
    }
}

struct SelectBuilderOption<'a, T> {
    title: &'static str,
    on_select: Box<dyn FnOnce() -> T + 'a>,
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
