use std::{
    borrow::Cow,
    fmt::{self, Arguments, Display, Formatter},
    fs,
    net::IpAddr,
    ops::Deref,
    str::FromStr,
};

use anyhow::Error;
use rand::seq::SliceRandom;

use crate::{
    context::{
        key::{Key, KeyOwned},
        kv::{self, Item, Value},
    },
    prelude::*,
};

pub const RAND_CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVXYZ\
                              abcdefghijklmnopqrstuvxyz\
                              0123456789";

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

#[throws(Error)]
pub fn get_attached_disks() -> Vec<DiskInfo> {
    match process!("uname").run()?.stdout.trim() {
        "Linux" => {
            let output = process!("lsblk -bOJ").run()?;
            serde_json::from_slice::<linux::LsblkOutput>(output.stdout.as_bytes())?.into()
        }
        "Darwin" => {
            let output = process!("diskutil list -plist external physical").run()?;
            plist::from_bytes::<macos::DiskutilOutput>(output.stdout.as_bytes())?.into()
        }
        os => bail!("Unsupported operating system: {os}"),
    }
}

fn unnamed_if_empty<S: AsRef<str> + ?Sized>(name: &S) -> String {
    if name.as_ref().trim().is_empty() {
        "<unnamed>".to_owned()
    } else {
        format!(r#""{}""#, name.as_ref())
    }
}

#[derive(Clone, PartialEq, Eq)]
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
        item.convert::<String>()?.parse().map_err(Error::from)?
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Opt<'a> {
    Yes,
    No,
    Modify,
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
            Self::Modify => write!(f, "Modify"),
            Self::Overwrite => write!(f, "Overwrite"),
            Self::Rerun => write!(f, "Rerun"),
            Self::Retry => write!(f, "Retry"),
            Self::Skip => write!(f, "Skip"),
            Self::Custom(s) => write!(f, "{s}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiskInfo {
    pub id: String,
    pub part_type: String,
    pub name: String,
    pub size: usize,
    pub partitions: Vec<DiskPartitionInfo>,
}

#[derive(Debug, Clone)]
pub struct DiskPartitionInfo {
    pub id: String,
    pub size: usize,
    pub name: String,
}

impl DiskInfo {
    pub fn description(&self) -> String {
        let mut desc = format!("{}: ", self.id);
        desc += &unnamed_if_empty(&self.name);
        if !self.partitions.is_empty() {
            desc += &format!(
                " ({} partition{}: {})",
                self.partitions.len(),
                if self.partitions.len() == 1 { "" } else { "s" },
                self.partitions
                    .iter()
                    .map(|p| unnamed_if_empty(&p.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        desc + &format!(", {:.2} GB", self.size as f64 / 1e9)
    }
}

impl Display for DiskInfo {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        self.description().fmt(f)
    }
}

impl DiskPartitionInfo {
    fn description(&self) -> String {
        format!(
            "{}: {} ({:.2} GB)",
            self.id,
            unnamed_if_empty(&self.name),
            self.size as f64 / 1e9,
        )
    }
}

impl Display for DiskPartitionInfo {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        self.description().fmt(f)
    }
}

mod linux {
    use serde::{Deserialize, Deserializer};

    use super::*;

    #[throws(D::Error)]
    fn nullable_field<'de, D, T>(deserializer: D) -> T
    where
        D: Deserializer<'de>,
        T: Deserialize<'de> + Default,
    {
        let opt = Option::<T>::deserialize(deserializer)?;
        opt.unwrap_or_default()
    }

    #[derive(Deserialize)]
    pub struct LsblkOutput {
        blockdevices: Vec<LsblkDisk>,
    }

    #[derive(Deserialize)]
    struct LsblkDisk {
        name: String,
        #[serde(deserialize_with = "nullable_field")]
        fstype: String,
        kname: String,
        size: usize,
        #[serde(default = "Vec::new")]
        children: Vec<LsblkPartition>,
    }

    #[derive(Deserialize)]
    struct LsblkPartition {
        name: String,
        label: String,
        size: usize,
    }

    impl From<LsblkOutput> for Vec<DiskInfo> {
        fn from(output: LsblkOutput) -> Self {
            output
                .blockdevices
                .into_iter()
                .map(DiskInfo::from)
                .collect()
        }
    }

    impl From<LsblkDisk> for DiskInfo {
        fn from(disk: LsblkDisk) -> Self {
            Self {
                id: disk.name,
                name: disk.kname,
                size: disk.size,
                part_type: disk.fstype,
                partitions: disk.children.into_iter().map(Into::into).collect(),
            }
        }
    }

    impl From<LsblkPartition> for DiskPartitionInfo {
        fn from(partition: LsblkPartition) -> Self {
            Self {
                id: partition.name,
                name: partition.label,
                size: partition.size,
            }
        }
    }
}

mod macos {
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct DiskutilOutput {
        all_disks_and_partitions: Vec<DiskutilDisk>,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct DiskutilDisk {
        device_identifier: String,
        #[serde(default = "String::new")]
        volume_name: String,
        size: usize,
        content: String,
        #[serde(default = "Vec::new")]
        partitions: Vec<DiskutilPartition>,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct DiskutilPartition {
        device_identifier: String,
        #[serde(default = "String::new")]
        volume_name: String,
        size: usize,
    }

    impl From<DiskutilOutput> for Vec<DiskInfo> {
        fn from(output: DiskutilOutput) -> Self {
            output
                .all_disks_and_partitions
                .into_iter()
                .map(DiskInfo::from)
                .collect()
        }
    }

    impl From<DiskutilDisk> for DiskInfo {
        fn from(disk: DiskutilDisk) -> Self {
            Self {
                id: disk.device_identifier,
                name: disk.volume_name,
                size: disk.size,
                part_type: disk.content,
                partitions: disk.partitions.into_iter().map(Into::into).collect(),
            }
        }
    }

    impl From<DiskutilPartition> for DiskPartitionInfo {
        fn from(partition: DiskutilPartition) -> Self {
            Self {
                id: partition.device_identifier,
                name: partition.volume_name,
                size: partition.size,
            }
        }
    }
}
