use std::{
    fmt::{self, Display, Formatter},
    net::{AddrParseError, IpAddr},
    num::ParseIntError,
    str::FromStr,
};

use derive_more::Display;
use serde::{de::Visitor, Deserialize, Serialize};
use smart_default::SmartDefault;
use thiserror::Error;

fn physical_disk_type() -> DiskType {
    DiskType::Physical
}

pub fn get_attached_disks<I: IntoIterator<Item = DiskType>>(
    disk_types: I,
) -> hoclog::Result<Vec<AttachedDiskInfo>> {
    let mut attached_disks_info = Vec::new();

    for disk_type in disk_types {
        let (_, stdout) = diskutil!("list", "-plist", "external", disk_type.as_ref())
            .hide_output()
            .run()?;

        let output: DiskutilOutput = plist::from_bytes(stdout.as_bytes()).unwrap();

        attached_disks_info.extend(output.all_disks_and_partitions.into_iter().map(|mut d| {
            d.disk_type = disk_type;
            d
        }))
    }

    Ok(attached_disks_info)
}

pub fn unnamed_if_empty<S: AsRef<str>>(name: S) -> String {
    if name.as_ref().trim().is_empty() {
        "<unnamed>".to_owned()
    } else {
        format!(r#""{}""#, name.as_ref())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, SmartDefault, Display, Serialize, Deserialize)]
pub enum Image {
    #[display(fmt = "Raspberry Pi OS ({_0})")]
    RaspberryPiOs(RaspberryPiOsVersion),
    #[default]
    #[display(fmt = "Ubuntu ({_0})")]
    Ubuntu(UbuntuVersion),
}

impl Image {
    pub fn url(&self) -> String {
        match self {
            Self::RaspberryPiOs(version) => format!("https://downloads.raspberrypi.org/raspios_lite_arm64/images/raspios_lite_arm64-{version}/{version}-raspios-bullseye-arm64-lite.zip"),
            Self::Ubuntu(version)=> format!("https://cdimage.ubuntu.com/releases/{version}/release/ubuntu-{version}-preinstalled-server-arm64+raspi.img.xz"),
        }
    }

    pub fn supported_versions() -> [Image; 2] {
        [
            Self::RaspberryPiOs(Default::default()),
            Self::Ubuntu(Default::default()),
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, SmartDefault, Display, Serialize, Deserialize)]
#[display(fmt = "{year:04}-{month:02}-{day:02}")]
pub struct RaspberryPiOsVersion {
    #[default = 2022]
    year: u32,
    #[default = 1]
    month: u32,
    #[default = 28]
    day: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, SmartDefault, Display, Serialize, Deserialize)]
#[display(fmt = "{major}.{minor:02}.{patch}")]
pub struct UbuntuVersion {
    #[default = 20]
    major: u32,
    #[default = 4]
    minor: u32,
    #[default = 4]
    patch: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DiskutilOutput {
    all_disks_and_partitions: Vec<AttachedDiskInfo>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AttachedDiskInfo {
    #[serde(rename = "DeviceIdentifier")]
    pub id: String,

    #[serde(rename = "Content")]
    pub part_type: String,

    #[serde(rename = "VolumeName", default = "String::new")]
    pub name: String,

    #[serde(skip_deserializing, default = "physical_disk_type")]
    pub disk_type: DiskType,

    pub size: usize,

    #[serde(default = "Vec::new")]
    pub partitions: Vec<AttachedDiskPartitionInfo>,
}

impl Display for AttachedDiskInfo {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        self.description().fmt(f)
    }
}

impl AttachedDiskInfo {
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AttachedDiskPartitionInfo {
    #[serde(rename = "DeviceIdentifier")]
    pub id: String,

    #[serde(rename = "Content", default = "String::new")]
    pub part_type: String,

    pub size: usize,

    #[serde(rename = "VolumeName", default = "String::new")]
    pub name: String,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum DiskType {
    Physical,
    Virtual,
}

impl AsRef<str> for DiskType {
    fn as_ref(&self) -> &str {
        match self {
            Self::Physical => "physical",
            Self::Virtual => "virtual",
        }
    }
}

pub struct Cidr {
    ip_addr: IpAddr,
    prefix_len: u32,
}

#[derive(Error, Debug)]
pub enum CidrParseError {
    #[error("expected '/' separator")]
    MissingSlash,

    #[error(transparent)]
    IpAddr(#[from] AddrParseError),

    #[error("prefix length is not a valid integer: {0}")]
    InvalidPrefixLen(#[from] ParseIntError),

    #[error("prefix length needs to be between 0 and {prefix_len_bound}, got {prefix_len}")]
    PrefixLenOutOfRange {
        prefix_len: u32,
        prefix_len_bound: u32,
    },
}

impl Display for Cidr {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let ip_addr = self.ip_addr;
        let prefix_len = self.prefix_len;
        write!(f, "{ip_addr}/{prefix_len}")
    }
}

impl FromStr for Cidr {
    type Err = CidrParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (ip_addr_str, prefix_len_str) = s
            .split_once("/")
            .ok_or_else(|| CidrParseError::MissingSlash)?;
        let ip_addr: IpAddr = ip_addr_str.parse()?;
        let prefix_len: u32 = prefix_len_str.parse()?;

        let prefix_len_bound = if ip_addr.is_ipv4() { 32 } else { 128 };

        if prefix_len <= prefix_len_bound {
            Ok(Cidr {
                ip_addr,
                prefix_len,
            })
        } else {
            Err(CidrParseError::PrefixLenOutOfRange {
                prefix_len,
                prefix_len_bound,
            })
        }
    }
}

impl Serialize for Cidr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Cidr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct CidrVisitor;

        impl<'de> Visitor<'de> for CidrVisitor {
            type Value = Cidr;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("CIDR block")
            }

            fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                s.parse().map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_str(CidrVisitor)
    }
}
