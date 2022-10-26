use std::{
    borrow::Cow,
    fmt::{self, Arguments, Display, Formatter},
    fs,
    ops::Deref,
    str::FromStr,
};

use crate::{
    context::key::{self, Key, KeyOwned},
    prelude::*,
};

pub fn from_arguments_to_str_cow(arguments: Arguments) -> Cow<'static, str> {
    if let Some(s) = arguments.as_str() {
        Cow::Borrowed(s)
    } else {
        Cow::Owned(arguments.to_string())
    }
}

#[throws(key::Error)]
pub fn try_from_arguments_to_key_cow(arguments: Arguments) -> Cow<'static, Key> {
    if let Some(s) = arguments.as_str() {
        Cow::Borrowed(Key::new(s)?)
    } else {
        Cow::Owned(KeyOwned::new(arguments.to_string())?)
    }
}

pub struct Secret<T>(T);

impl<T> Secret<T> {
    pub fn new(inner: T) -> Self {
        Self(inner)
    }
}

impl<T> Deref for Secret<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> Display for Secret<T> {
    #[throws(fmt::Error)]
    fn fmt(&self, f: &mut Formatter) {
        write!(f, "********")?
    }
}

impl<T> FromStr for Secret<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    type Err = anyhow::Error;

    #[throws(Self::Err)]
    fn from_str(path: &str) -> Self {
        let secret = fs::read_to_string(path)?;
        Secret(T::from_str(&secret)?)
    }
}
