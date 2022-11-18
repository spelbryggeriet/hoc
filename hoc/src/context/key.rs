use std::{
    borrow::{Borrow, Cow},
    fmt::{self, Display, Formatter},
    ops::Deref,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::prelude::*;

#[derive(PartialEq, Eq, Hash)]
pub struct Key {
    inner: str,
}

impl Key {
    pub fn new<P: AsRef<str> + ?Sized>(key: &P) -> &Self {
        let stripped = key.as_ref().trim_start_matches('/').trim_end_matches('/');
        unsafe { &*(stripped as *const str as *const Key) }
    }

    pub fn empty() -> &'static Self {
        Self::new("")
    }

    pub fn contains_wildcard(&self) -> bool {
        self.inner.contains('*')
    }

    pub fn components(&self) -> Components {
        Components {
            inner: self.inner.split('/'),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.inner
    }

    pub fn join<K: AsRef<Key> + ?Sized>(&self, other: &K) -> KeyOwned {
        KeyOwned {
            inner: self.inner.to_owned() + "/" + &other.as_ref().inner,
        }
    }
}

impl ToOwned for Key {
    type Owned = KeyOwned;

    fn to_owned(&self) -> Self::Owned {
        KeyOwned {
            inner: self.inner.to_owned(),
        }
    }
}

impl Display for Key {
    #[throws(fmt::Error)]
    fn fmt(&self, f: &mut Formatter) {
        write!(f, "{:?}", &self.inner)?;
    }
}

impl AsRef<Key> for Key {
    fn as_ref(&self) -> &Key {
        self
    }
}

impl AsRef<Key> for str {
    fn as_ref(&self) -> &Key {
        Key::new(self)
    }
}

impl AsRef<Key> for String {
    fn as_ref(&self) -> &Key {
        Key::new(self)
    }
}

impl AsRef<Key> for Cow<'_, str> {
    fn as_ref(&self) -> &Key {
        Key::new(self)
    }
}

impl AsRef<Key> for KeyComponent<'_> {
    fn as_ref(&self) -> &Key {
        Key::new(self.as_str())
    }
}

impl<'a> From<&'a str> for &'a Key {
    fn from(s: &'a str) -> Self {
        Key::new(s)
    }
}

impl<'a> From<&'a String> for &'a Key {
    fn from(s: &'a String) -> Self {
        Key::new(s)
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct KeyOwned {
    inner: String,
}

impl KeyOwned {
    pub fn new() -> Self {
        Self {
            inner: String::new(),
        }
    }
}

impl Borrow<Key> for KeyOwned {
    fn borrow(&self) -> &Key {
        Key::new(&self.inner)
    }
}

impl Deref for KeyOwned {
    type Target = Key;

    fn deref(&self) -> &Self::Target {
        Key::new(&self.inner)
    }
}

impl Display for KeyOwned {
    #[throws(fmt::Error)]
    fn fmt(&self, f: &mut Formatter) {
        Key::fmt(&*self, f)?;
    }
}

impl AsRef<Key> for KeyOwned {
    fn as_ref(&self) -> &Key {
        self
    }
}

impl<T: AsRef<Key> + ?Sized> From<&T> for KeyOwned {
    fn from(key: &T) -> Self {
        Self {
            inner: key.as_ref().as_str().to_owned(),
        }
    }
}
impl From<String> for KeyOwned {
    fn from(key: String) -> Self {
        Self { inner: key }
    }
}

impl<'a> From<Cow<'a, Key>> for KeyOwned {
    fn from(key: Cow<'a, Key>) -> Self {
        key.into_owned()
    }
}

impl<'a> FromIterator<KeyComponent<'a>> for KeyOwned {
    fn from_iter<T: IntoIterator<Item = KeyComponent<'a>>>(iter: T) -> Self {
        let mut key = KeyOwned::new();
        for component in iter.into_iter() {
            key = key.join(&component);
        }
        key
    }
}

pub struct Components<'a> {
    inner: std::str::Split<'a, char>,
}

impl<'a> Iterator for Components<'a> {
    type Item = KeyComponent<'a>;

    #[throws(as Option)]
    fn next(&mut self) -> Self::Item {
        loop {
            let inner = self.inner.next()?;
            if !inner.is_empty() {
                break KeyComponent::new(inner);
            }
        }
    }
}

impl DoubleEndedIterator for Components<'_> {
    #[throws(as Option)]
    fn next_back(&mut self) -> Self::Item {
        loop {
            let inner = self.inner.next_back()?;
            if !inner.is_empty() {
                break KeyComponent::new(inner);
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct KeyComponent<'a> {
    inner: &'a str,
}

impl<'a> KeyComponent<'a> {
    pub fn new(component: &'a str) -> Self {
        Self { inner: component }
    }

    pub fn is_flat_wildcard(&self) -> bool {
        self.inner == "*"
    }

    pub fn is_nested_wildcard(&self) -> bool {
        self.inner == "**"
    }

    pub fn as_str(&self) -> &str {
        self.inner
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error(r#"Key already exists: "{0}""#)]
    KeyAlreadyExists(KeyOwned),

    #[error(r#"Key does not exist: "{0}""#)]
    KeyDoesNotExist(KeyOwned),

    #[error(r#"Unexpected leading `/` in key: "{0}"#)]
    LeadingForwardSlash(KeyOwned),

    #[error(r#"Unexpected `.` in key: "{0}"#)]
    SingleDotComponent(String),

    #[error(r#"Unexpected `..` in key: "{0}""#)]
    DoubleDotComponent(String),
}
