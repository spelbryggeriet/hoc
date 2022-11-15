use std::{
    borrow::{Borrow, Cow},
    fmt::{self, Display, Formatter},
    ops::Deref,
    path::{Component, Path, PathBuf},
};

use thiserror::Error;

use crate::prelude::*;

#[throws(Error)]
fn check_key(key: &Key) {
    if key.is_absolute() {
        throw!(Error::LeadingForwardSlash(key.to_owned()));
    }

    for comp in key.components() {
        match comp {
            Component::CurDir => throw!(Error::SingleDotComponent(key.to_owned())),
            Component::ParentDir => {
                throw!(Error::DoubleDotComponent(key.to_owned()))
            }
            _ => (),
        }
    }
}

pub struct Key {
    inner: Path,
}

impl Key {
    #[throws(Error)]
    pub fn new<P: AsRef<Path> + ?Sized>(key: &P) -> &Self {
        let unchecked = Self::new_unchecked(key);
        check_key(unchecked)?;
        unchecked
    }

    pub(super) fn new_unchecked<P: AsRef<Path> + ?Sized>(key: &P) -> &Self {
        unsafe { &*(key.as_ref() as *const Path as *const Key) }
    }
}

impl ToOwned for Key {
    type Owned = KeyOwned;

    fn to_owned(&self) -> Self::Owned {
        KeyOwned::new_unchecked(self.inner.to_owned())
    }
}

impl Deref for Key {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Display for Key {
    #[throws(fmt::Error)]
    fn fmt(&self, f: &mut Formatter) {
        write!(f, "{:?}", &self.inner)?;
    }
}

#[derive(Debug)]
pub struct KeyOwned {
    inner: PathBuf,
}

impl KeyOwned {
    #[throws(Error)]
    pub fn new<P: Into<PathBuf>>(key: P) -> Self {
        let unchecked = Self::new_unchecked(key.into());
        check_key(&unchecked)?;
        unchecked
    }

    pub(super) fn new_unchecked<P: Into<PathBuf>>(path_buf: P) -> Self {
        Self {
            inner: path_buf.into(),
        }
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.inner
    }
}

impl Borrow<Key> for KeyOwned {
    fn borrow(&self) -> &Key {
        Key::new_unchecked(self.inner.as_path())
    }
}

impl Deref for KeyOwned {
    type Target = Key;

    fn deref(&self) -> &Self::Target {
        Key::new_unchecked(&self.inner)
    }
}

impl Display for KeyOwned {
    #[throws(fmt::Error)]
    fn fmt(&self, f: &mut Formatter) {
        Key::fmt(&**self, f)?;
    }
}

impl<'a> From<&'a Key> for Cow<'a, Key> {
    fn from(key: &'a Key) -> Self {
        Cow::Borrowed(key)
    }
}

impl From<KeyOwned> for Cow<'_, Key> {
    fn from(key: KeyOwned) -> Self {
        Cow::Owned(key)
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
    SingleDotComponent(KeyOwned),

    #[error(r#"Unexpected `..` in key: "{0}""#)]
    DoubleDotComponent(KeyOwned),
}
