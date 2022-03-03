use std::fmt::{self, Display, Formatter};

use derive_more::Display;
use serde::{Deserialize, Serialize};
use smart_default::SmartDefault;

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
