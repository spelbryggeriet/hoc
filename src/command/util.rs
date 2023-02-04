use std::fmt::{self, Display, Formatter};

use clap::Parser;

use crate::prelude::*;

#[derive(Parser)]
pub struct Defaults {
    /// Skip prompts for fields that have defaults
    ///
    /// This is equivalent to providing all defaultable flags without a value.
    #[clap(short, long)]
    defaults: bool,
}

#[throws(anyhow::Error)]
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
        #[serde(deserialize_with = "nullable_field")]
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
