use std::{
    fmt::{Debug, Display},
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
    str::FromStr,
};

use async_std::task;
use async_trait::async_trait;
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

#[throws(Error)]
async fn get_prompt<T>(field: &str, default: Option<&str>) -> T
where
    T: FromStr,
    T::Err: Display,
{
    let default_owned = default.map(<str>::to_string);
    let prompt = format!("{field}:");

    let prompt_fut = task::spawn_blocking(move || {
        let _pause_lock = logger::render::pause()?;

        let default_clone = default_owned.clone();
        let mut text =
            Text::new(&prompt)
                .with_validator(move |s: &str| {
                    let s = s.trim();
                    if s.is_empty() {
                        return if let Some(ref default) = default_clone {
                            match T::from_str(default) {
                                Ok(_) => Ok(Validation::Valid),
                                Err(_) => Err(Box::new(InvalidDefaultError(default.clone()))
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

        if let Some(ref default) = default_owned {
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
                if let Some(default) = default {
                    T::from_str(default).unwrap_or_else(|_| unreachable!())
                } else {
                    throw!(err)
                }
            }
        },
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
    pub async fn get(self) -> T {
        task::spawn_blocking(move || {
            let _pause_lock = logger::render::pause()?;

            let resp = Select::new(&self.message, self.options)
                .with_formatter(&|_| clear_prompt())
                .prompt()?;

            Result::<_, Error>::Ok(resp)
        })
        .await?
    }
}

pub type SelectBuilderFuture<T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'static>>;

impl<T: Display + Send + 'static> IntoFuture for SelectBuilder<T, NonEmpty> {
    type IntoFuture = SelectBuilderFuture<T>;
    type Output = <SelectBuilderFuture<T> as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.get())
    }
}

#[derive(Debug, Error)]
#[error("Invalid default value: {0}")]
struct InvalidDefaultError(String);

#[async_trait]
pub trait Prompt<T: FromStr>: private::Sealed {
    async fn get(self, field: &str) -> Result<T, Error>;

    async fn get_or(self, field: &str, default: &str) -> Result<T, Error>;
}

#[async_trait]
impl<T> Prompt<T> for Option<T>
where
    T: FromStr + ToString + Send,
    T::Err: Display,
{
    async fn get(self, field: &str) -> Result<T, Error> {
        if let Some(inner) = self {
            info!("{field}: {}", inner.to_string());
            Ok(inner)
        } else {
            let value: T = get_prompt(field, None).await?;
            info!("{field}: {}", value.to_string());
            Ok(value)
        }
    }

    async fn get_or(self, field: &str, default: &str) -> Result<T, Error> {
        if let Some(inner) = self {
            info!("{field}: {}", inner.to_string());
            Ok(inner)
        } else {
            let value: T = get_prompt(field, Some(default)).await?;
            info!("{field}: {}", value.to_string());
            Ok(value)
        }
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Render(#[from] logger::render::Error),

    #[error(transparent)]
    Inquire(#[from] inquire::InquireError),
}

impl<T> private::Sealed for Option<T> {}

mod private {
    pub trait Sealed {}
}
