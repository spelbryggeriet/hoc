use std::{
    borrow::{Borrow, Cow},
    ffi::OsStr,
    fmt::{self, Display, Formatter},
    ops::Deref,
    os::unix::prelude::OsStrExt,
    path::{self, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::prelude::*;

#[throws(Error)]
fn check_key(key: &Key) {
    if key.inner.is_absolute() {
        throw!(Error::LeadingForwardSlash(key.to_owned()));
    }

    key.components()
        .map(check_key_component)
        .collect::<Result<_, _>>()?;
}

#[throws(Error)]
fn check_key_component(key_component: KeyComponent) {
    match key_component.inner {
        path::Component::CurDir => throw!(Error::SingleDotComponent(
            key_component
                .inner
                .as_os_str()
                .to_string_lossy()
                .into_owned()
        )),
        path::Component::ParentDir => {
            throw!(Error::DoubleDotComponent(
                key_component
                    .inner
                    .as_os_str()
                    .to_string_lossy()
                    .into_owned()
            ))
        }
        _ => (),
    }
}

#[derive(PartialEq, Eq, Hash)]
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

    pub fn new_unchecked<P: AsRef<Path> + ?Sized>(key: &P) -> &Self {
        unsafe { &*(key.as_ref() as *const Path as *const Key) }
    }

    pub fn contains_wildcard(&self) -> bool {
        self.inner.as_os_str().as_bytes().contains(&b'*')
    }

    pub fn components(&self) -> Components {
        Components {
            inner: self.inner.components(),
        }
    }

    pub fn to_string_lossy(&self) -> Cow<str> {
        self.inner.as_os_str().to_string_lossy()
    }
}

impl ToOwned for Key {
    type Owned = KeyOwned;

    fn to_owned(&self) -> Self::Owned {
        KeyOwned::new_unchecked(self.inner.to_owned())
    }
}

impl Display for Key {
    #[throws(fmt::Error)]
    fn fmt(&self, f: &mut Formatter) {
        write!(f, "{:?}", &self.inner)?;
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
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

    pub fn new_unchecked<P: Into<PathBuf>>(path_buf: P) -> Self {
        Self {
            inner: path_buf.into(),
        }
    }

    pub fn empty() -> Self {
        Self::new_unchecked("")
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

pub struct Components<'a> {
    inner: path::Components<'a>,
}

impl<'a> Iterator for Components<'a> {
    type Item = KeyComponent<'a>;

    #[throws(as Option)]
    fn next(&mut self) -> Self::Item {
        KeyComponent {
            inner: self.inner.next()?,
        }
    }
}

impl DoubleEndedIterator for Components<'_> {
    #[throws(as Option)]
    fn next_back(&mut self) -> Self::Item {
        KeyComponent {
            inner: self.inner.next_back()?,
        }
    }
}

#[derive(Clone, Copy)]
pub struct KeyComponent<'a> {
    inner: path::Component<'a>,
}

impl<'a> KeyComponent<'a> {
    #[throws(Error)]
    pub fn new<P: AsRef<OsStr>>(component: &'a P) -> Self {
        let component = Self {
            inner: path::Component::Normal(component.as_ref()),
        };
        check_key_component(component)?;
        component
    }

    pub fn is_flat_wildcard(&self) -> bool {
        self.inner.as_os_str() == "*"
    }

    pub fn is_nested_wildcard(&self) -> bool {
        self.inner.as_os_str() == "**"
    }

    pub fn to_string_lossy(&self) -> Cow<str> {
        self.inner.as_os_str().to_string_lossy()
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
