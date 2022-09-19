use std::{
    fmt::{Debug, Display},
    str::FromStr,
};

use crossterm::{
    cursor::MoveToPreviousLine,
    execute,
    terminal::{Clear, ClearType},
};
use inquire::{
    error::CustomUserError,
    validator::{ErrorMessage, Validation},
    InquireError, Text,
};
use thiserror::Error;

use crate::prelude::*;

#[throws(InquireError)]
fn get_prompt<T>(field: &str, default: Option<&str>) -> T
where
    T: FromStr,
    T::Err: Display,
{
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
        .with_formatter(&|_| {
            let mut control_sequence = Vec::new();
            execute!(
                control_sequence,
                Clear(ClearType::CurrentLine),
                MoveToPreviousLine(1),
            )
            .unwrap();
            String::from_utf8(control_sequence).unwrap()
        });

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
