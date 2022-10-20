use std::{
    borrow::Cow,
    fmt::{Debug, Display},
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
    str::FromStr,
};

use async_std::task;
use crossterm::{
    cursor::{self, MoveToPreviousLine},
    terminal::{Clear, ClearType},
};
use inquire::{
    error::CustomUserError,
    ui::{Color, RenderConfig, StyleSheet, Styled},
    validator::{ErrorMessage, Validation},
    Password, PasswordDisplayMode, Select, Text,
};
use thiserror::Error;

use crate::{logger, prelude::*, util::Secret};

fn clear_prompt() -> String {
    use std::fmt::Write;

    let mut clear_section = String::new();
    write!(
        clear_section,
        "{clear_line}{move_line}{move_column}",
        clear_line = Clear(ClearType::CurrentLine),
        move_line = MoveToPreviousLine(2),
        move_column = cursor::MoveToColumn(0),
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
    async fn get(self) -> T {
        let default_clone = self.default.clone();
        let prompt = format!("{}:", self.message);

        let prompt_fut = task::spawn_blocking(move || {
            let _pause_lock = logger::pause()?;

            let default_clone_2 = default_clone.clone();
            let validator = move |s: &str| {
                let s = s.trim();
                if s.is_empty() {
                    return if let Some(default) = &default_clone_2 {
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
                .with_formatter(&|_| clear_prompt());

            if let Some(ref default) = default_clone {
                text = text.with_default(default);
            }

            let resp = text.prompt()?;

            Ok(resp)
        });

        match prompt_fut.await {
            Ok(resp) => T::from_str(resp.trim()).unwrap_or_else(|_| unreachable!()),
            Err(Error::Render(err)) => throw!(err),
            Err(Error::Inquire(err)) => match err {
                err @ (inquire::InquireError::Custom(_)
                | inquire::InquireError::OperationCanceled
                | inquire::InquireError::OperationInterrupted) => throw!(err),
                err => {
                    if let Some(default) = &self.default {
                        T::from_str(default).unwrap_or_else(|_| unreachable!())
                    } else {
                        throw!(err)
                    }
                }
            },
        }
    }
}

impl<T> PromptBuilder<T, AsSecret>
where
    T: FromStr,
    T::Err: Display,
{
    #[throws(Error)]
    async fn get(self) -> Secret<T> {
        let prompt = format!("{}:", self.message);

        let prompt_fut = task::spawn_blocking(move || {
            let _pause_lock = logger::pause()?;

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
                .with_display_mode(PasswordDisplayMode::Masked)
                .with_display_toggle_enabled()
                .with_help_message("Ctrl-R to reveal/hide")
                .with_validator(validator)
                .with_formatter(&|_| clear_prompt());

            let resp = text.prompt()?;

            Result::<_, Error>::Ok(resp)
        });

        let resp = prompt_fut.await?;

        Secret::new(T::from_str(resp.trim()).unwrap_or_else(|_| unreachable!()))
    }
}

pub type BuilderFuture<T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'static>>;

impl<T, S> IntoFuture for PromptBuilder<T, S>
where
    T: FromStr + Send + 'static,
    T::Err: Display,
    S: NonSecret + Send + 'static,
{
    type IntoFuture = BuilderFuture<T>;
    type Output = <BuilderFuture<T> as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.get())
    }
}

impl<T> IntoFuture for PromptBuilder<T, AsSecret>
where
    T: FromStr + Send + 'static,
    T::Err: Display,
{
    type IntoFuture = BuilderFuture<Secret<T>>;
    type Output = <BuilderFuture<Secret<T>> as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.get())
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
    pub async fn get(self) -> T {
        task::spawn_blocking(move || {
            let _pause_lock = logger::pause()?;

            let resp = Select::new(&self.message, self.options)
                .with_formatter(&|_| clear_prompt())
                .prompt()?;

            Result::<_, Error>::Ok(resp)
        })
        .await?
    }
}

impl<T: Display + Send + 'static> IntoFuture for SelectBuilder<T, NonEmpty> {
    type IntoFuture = BuilderFuture<T>;
    type Output = <BuilderFuture<T> as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.get())
    }
}
#[derive(Debug, Error)]
#[error("Invalid default value: {0}")]
pub struct InvalidDefaultError(String);

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Render(#[from] logger::RenderError),

    #[error(transparent)]
    Inquire(#[from] inquire::InquireError),
}

impl<T> private::Sealed for Option<T> {}

mod private {
    pub trait Sealed {}
}
