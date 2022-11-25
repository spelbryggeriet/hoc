use std::{
    borrow::Cow,
    fmt::{self, Arguments, Display, Formatter},
    fs,
    net::IpAddr,
    ops::Deref,
    str::FromStr,
};

use rand::seq::SliceRandom;

use crate::{
    context::{
        key::{Key, KeyOwned},
        kv::{self, Item, Value},
    },
    prelude::*,
};

pub fn from_arguments_to_str_cow(arguments: Arguments) -> Cow<'static, str> {
    if let Some(s) = arguments.as_str() {
        Cow::Borrowed(s)
    } else {
        Cow::Owned(arguments.to_string())
    }
}

pub fn from_arguments_to_key_cow(arguments: Arguments) -> Cow<'static, Key> {
    if let Some(s) = arguments.as_str() {
        Cow::Borrowed(Key::new(s))
    } else {
        Cow::Owned(KeyOwned::default().join(&arguments.to_string()))
    }
}

pub fn numeral(n: u64) -> Cow<'static, str> {
    match n {
        0 => "zero".into(),
        1 => "one".into(),
        2 => "two".into(),
        3 => "three".into(),
        4 => "four".into(),
        5 => "five".into(),
        6 => "six".into(),
        7 => "seven".into(),
        8 => "eight".into(),
        9 => "nine".into(),
        10 => "ten".into(),
        11 => "eleven".into(),
        12 => "twelve".into(),
        13 => "thirteen".into(),
        14 => "fourteen".into(),
        15 => "fifteen".into(),
        16 => "sixteen".into(),
        17 => "seventeen".into(),
        18 => "eighteen".into(),
        19 => "nineteen".into(),
        20 => "twenty".into(),
        30 => "thirty".into(),
        40 => "fourty".into(),
        50 => "fifty".into(),
        60 => "sixty".into(),
        70 => "seventy".into(),
        80 => "eighty".into(),
        90 => "ninety".into(),
        100 => "hundred".into(),
        1000 => "thousand".into(),
        1_000_000 => "million".into(),
        n if n <= 99 => format!("{}-{}", numeral(n - n % 10), numeral(n % 10)).into(),
        n if n <= 199 => format!("hundred-{}", numeral(n % 100)).into(),
        n if n <= 999 && n % 100 == 0 => format!("{}-hundred", numeral(n / 100)).into(),
        n if n <= 999 => format!("{}-{}", numeral(n - n % 100), numeral(n % 100)).into(),
        n if n <= 1999 => format!("thousand-{}", numeral(n % 1000)).into(),
        n if n <= 999_999 && n % 1000 == 0 => format!("{}-thousand", numeral(n / 1000)).into(),
        n if n <= 999_999 => format!("{}-{}", numeral(n - n % 1000), numeral(n % 1000)).into(),
        n if n <= 1_999_999 => format!("million-{}", numeral(n % 1_000_000)).into(),
        n if n % 1_000_000 == 0 => format!("{}-million", numeral(n / 1_000_000)).into(),

        mut n => {
            let mut list = Vec::new();
            loop {
                list.push(numeral(n % 1_000_000));
                n /= 1_000_000;
                if n == 0 {
                    break;
                }
                list.push("million".into());
            }
            list.reverse();
            list.join("-").into()
        }
    }
}

pub fn random_string(source: &str, len: usize) -> String {
    let mut rng = rand::thread_rng();
    let sample: Vec<char> = source.chars().collect();
    sample.choose_multiple(&mut rng, len).collect()
}

#[derive(Clone)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    pub fn new(inner: T) -> Self {
        Self(inner)
    }

    pub fn into_non_secret(self) -> T {
        self.0
    }
}

impl<T: Deref> Secret<T> {
    pub fn as_deref(&self) -> Secret<&<T as Deref>::Target> {
        Secret(&self.0)
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
    T::Err: Into<anyhow::Error>,
{
    type Err = anyhow::Error;

    #[throws(Self::Err)]
    fn from_str(path: &str) -> Self {
        let secret = fs::read_to_string(path)?;
        Secret(T::from_str(&secret).map_err(Into::into)?)
    }
}

impl<T> From<Secret<T>> for Value
where
    Self: From<T>,
{
    fn from(secret: Secret<T>) -> Self {
        Self::from(secret.0)
    }
}

impl TryFrom<Item> for IpAddr {
    type Error = kv::Error;

    #[throws(Self::Error)]
    fn try_from(item: Item) -> Self {
        item.convert::<String>()?
            .parse()
            .map_err(anyhow::Error::from)?
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Opt<'a> {
    Yes,
    No,
    Overwrite,
    Rerun,
    Retry,
    Skip,
    Custom(&'a str),
}

impl Display for Opt<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Self::Yes => write!(f, "Yes"),
            Self::No => write!(f, "No"),
            Self::Overwrite => write!(f, "Overwrite"),
            Self::Rerun => write!(f, "Rerun"),
            Self::Retry => write!(f, "Retry"),
            Self::Skip => write!(f, "Skip"),
            Self::Custom(s) => write!(f, "{s}"),
        }
    }
}