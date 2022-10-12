use std::{
    fmt::{Debug, Display},
    marker::PhantomData,
    str::FromStr,
};

use crossterm::{
    cursor::{self, MoveToPreviousLine},
    terminal::{Clear, ClearType},
};
use inquire::{
    error::CustomUserError,
    validator::{ErrorMessage, Validation},
    InquireError, Select, Text,
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

#[throws(InquireError)]
fn get_prompt<T>(field: &str, default: Option<&str>) -> T
where
    T: FromStr,
    T::Err: Display,
{
    let _pause_lock = logger::pause();

    let default_owned = default.map(<str>::to_string);
    let prompt = format!("{field}:");
    let mut text = Text::new(&prompt)
        .with_validator(move |s: &str| {
            let s = s.trim();
            if s.is_empty() {
                return if let Some(ref default) = default_owned {
                    match T::from_str(default) {
                        Ok(_) => Ok(Validation::Valid),
                        Err(_) => {
                            Err(Box::new(InvalidDefaultError(default.clone())) as CustomUserError)
                        }
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

    if let Some(default) = default {
        text = text.with_default(default);
    }

    match text.prompt() {
        Ok(resp) => T::from_str(resp.trim()).unwrap_or_else(|_| unreachable!()),
        Err(
            err @ (InquireError::Custom(_)
            | InquireError::OperationCanceled
            | InquireError::OperationInterrupted),
        ) => throw!(err),
        Err(err) => {
            if let Some(default) = default {
                T::from_str(default).unwrap_or_else(|_| unreachable!())
            } else {
                throw!(err)
            }
        }
    }
}

pub fn select<'msg, T>(message: &'msg str) -> SelectBuilder<'msg, T, Empty> {
    SelectBuilder {
        message,
        options: Vec::with_capacity(1),
        _state: Default::default(),
    }
}

pub struct SelectBuilder<'msg, T, S> {
    message: &'msg str,
    options: Vec<T>,
    _state: PhantomData<S>,
}

pub enum Empty {}
pub enum NonEmpty {}

impl<'msg, T, S> SelectBuilder<'msg, T, S> {
    fn into_non_empty(self) -> SelectBuilder<'msg, T, NonEmpty> {
        SelectBuilder {
            message: self.message,
            options: self.options,
            _state: Default::default(),
        }
    }

    pub fn with_option(mut self, option: T) -> SelectBuilder<'msg, T, NonEmpty> {
        self.options.push(option);
        self.into_non_empty()
    }

    pub fn with_options(
        mut self,
        options: impl IntoIterator<Item = T>,
    ) -> SelectBuilder<'msg, T, NonEmpty> {
        self.options.extend(options);
        self.into_non_empty()
    }
}

impl<'msg, T: Display> SelectBuilder<'msg, T, NonEmpty> {
    #[throws(InquireError)]
    pub fn get(self) -> T {
        Select::new(self.message, self.options)
            .with_formatter(&|_| clear_prompt())
            .prompt()?
    }
}

#[derive(Debug, Error)]
#[error("Invalid default value: {0}")]
struct InvalidDefaultError(String);

pub trait Prompt<T: FromStr>: private::Sealed {
    #[throws(InquireError)]
    fn get(self, field: &str) -> T;

    #[throws(InquireError)]
    fn get_or(self, field: &str, default: &str) -> T;
}

impl<T> Prompt<T> for Option<T>
where
    T: FromStr + ToString,
    T::Err: Display,
{
    #[throws(InquireError)]
    fn get(self, field: &str) -> T {
        if let Some(inner) = self {
            info!("{field}: {}", inner.to_string());
            inner
        } else {
            let value: T = get_prompt(field, None)?;
            info!("{field}: {}", value.to_string());
            value
        }
    }

    #[throws(InquireError)]
    fn get_or(self, field: &str, default: &str) -> T {
        if let Some(inner) = self {
            info!("{field}: {}", inner.to_string());
            inner
        } else {
            let value: T = get_prompt(field, Some(default))?;
            info!("{field}: {}", value.to_string());
            value
        }
    }
}

impl<T> private::Sealed for Option<T> {}

mod private {
    pub trait Sealed {}
}
