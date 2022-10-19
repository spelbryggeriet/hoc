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
    validator::{ErrorMessage, Validation},
    Select, Text,
};
use thiserror::Error;

use crate::{logger, prelude::*};

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

pub struct PromptBuilder<T> {
    message: Cow<'static, str>,
    default: Option<Cow<'static, str>>,
    _output_type: PhantomData<T>,
}

impl<T> PromptBuilder<T> {
    pub fn new(message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            message: message.into(),
            default: None,
            _output_type: Default::default(),
        }
    }

    pub fn with_default(mut self, default: impl Into<Cow<'static, str>>) -> Self {
        self.default.replace(default.into());
        self
    }
}

impl<T> PromptBuilder<T>
where
    T: FromStr,
    T::Err: Display,
{
    #[throws(Error)]
    async fn get(self) -> T {
        let default_clone = self.default.clone();
        let prompt = format!("{}:", self.message);

        let prompt_fut = task::spawn_blocking(move || {
            let _pause_lock = logger::pause()?;

            let default_clone_2 = default_clone.clone();
            let mut text = Text::new(&prompt)
                .with_validator(move |s: &str| {
                    let s = s.trim();
                    if s.is_empty() {
                        return if let Some(ref default) = default_clone_2 {
                            match T::from_str(default.as_ref()) {
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
                })
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
                    if let Some(default) = self.default {
                        T::from_str(default.as_ref()).unwrap_or_else(|_| unreachable!())
                    } else {
                        throw!(err)
                    }
                }
            },
        }
    }
}

pub type BuilderFuture<T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'static>>;

impl<T> IntoFuture for PromptBuilder<T>
where
    T: FromStr + Send + 'static,
    T::Err: Display,
{
    type IntoFuture = BuilderFuture<T>;
    type Output = <BuilderFuture<T> as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.get())
    }
}

pub fn select<T>(message: String) -> SelectBuilder<T, Empty> {
    SelectBuilder {
        message,
        options: Vec::with_capacity(1),
        _state: Default::default(),
    }
}

pub struct SelectBuilder<T, S> {
    message: String,
    options: Vec<T>,
    _state: PhantomData<S>,
}

pub enum Empty {}
pub enum NonEmpty {}

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
struct InvalidDefaultError(String);

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
