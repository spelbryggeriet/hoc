use std::{
    fmt::{self, Display, Formatter},
    path::{Path, PathBuf},
};

use hoc_log::{error, info, status, Result};
use hoc_macros::define_commands;
use serde::Deserialize;

#[define_commands(diskutil)]
pub fn get_attached_disks() -> Result<impl Iterator<Item = AttachedDiskInfo>> {
    let (_, stdout) = diskutil!("list -plist external {}", Type::Physical)
        .hide_output()
        .run()?;

    let output: DiskutilOutput = plist::from_bytes(stdout.as_bytes()).unwrap();

    Ok(output
        .all_disks_and_partitions
        .into_iter()
        .filter(|d| d.part_type == "FDisk_partition_scheme"))
}

pub fn get_attached_disk_partitions() -> Result<impl Iterator<Item = AttachedDiskPartitionInfo>> {
    Ok(get_attached_disks()?.flat_map(|d| d.partitions))
}

#[define_commands(df)]
pub fn find_mount_dir(disk_id: &str) -> Result<PathBuf> {
    status!("Find mount directory image disk").on(|| {
        info!("Disk ID: {}", disk_id);
        let (_, df_output) = df!().run()?;
        let disk_line =
            if let Some(disk_line) = df_output.lines().find(|line| line.contains(&disk_id)) {
                disk_line
            } else {
                error!("{} not mounted", disk_id)?.into()
            };

        if let Some(mount_dir) = disk_line.split_terminator(' ').last() {
            Ok(Path::new(mount_dir).to_path_buf())
        } else {
            error!("mount point not found for {}", disk_id)?.into()
        }
    })
}

pub fn unnamed_if_empty<S: AsRef<str>>(name: S) -> String {
    if name.as_ref().trim().is_empty() {
        "<unnamed>".to_owned()
    } else {
        format!(r#""{}""#, name.as_ref())
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Type {
    Physical,
}

impl Display for Type {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Self::Physical => write!(f, "physical"),
        }
    }
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

impl Display for AttachedDiskPartitionInfo {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        self.description().fmt(f)
    }
}

impl AttachedDiskPartitionInfo {
    pub fn description(&self) -> String {
        format!(
            "{}: {} ({:.2} GB)",
            self.id,
            unnamed_if_empty(&self.name),
            self.size as f64 / 1e9,
        )
    }
}
