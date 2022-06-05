use std::{
    fmt::{self, Display, Formatter},
    path::{Path, PathBuf},
    result::Result as StdResult,
};

use hoc_core::process::{self, ssh};
use hoc_log::{bail, error, info, LogErr, Result};
use hoc_macros::{define_commands, doc_status};
use serde::{Deserialize, Deserializer};

#[define_commands(diskutil, lsblk, uname)]
pub fn get_attached_disks(
    ssh_client: Option<&ssh::Client>,
) -> Result<impl Iterator<Item = DiskInfo>> {
    let mut common_set = process::Settings::default();
    if let Some(client) = ssh_client {
        common_set = common_set.ssh(client)
    }
    let hidden_set = common_set.clone().hide_output();

    let info: Vec<DiskInfo> = match &*uname!().run_with(&common_set)?.1 {
        "Linux" => {
            let (_, stdout) = lsblk!("-bOJ").run_with(&hidden_set)?;
            let output: LsblkOutput =
                serde_json::from_slice(stdout.as_bytes()).log_context("parsing lsblk output")?;
            output
                .blockdevices
                .into_iter()
                .map(|disk| DiskInfo {
                    id: disk.name,
                    name: disk.kname,
                    size: disk.size,
                    part_type: disk.fstype,
                    partitions: disk
                        .children
                        .into_iter()
                        .map(|part| DiskPartitionInfo {
                            id: part.name,
                            name: part.label,
                            size: part.size,
                        })
                        .collect(),
                })
                .collect()
        }
        "Darwin" => {
            let (_, stdout) =
                diskutil!("list -plist external {}", Type::Physical).run_with(&hidden_set)?;
            let output: DiskutilOutput =
                plist::from_bytes(stdout.as_bytes()).log_context("parsing diskutil output")?;
            output
                .all_disks_and_partitions
                .into_iter()
                .map(|disk| DiskInfo {
                    id: disk.device_identifier,
                    name: disk.volume_name,
                    size: disk.size,
                    part_type: disk.content,
                    partitions: disk
                        .partitions
                        .into_iter()
                        .map(|part| DiskPartitionInfo {
                            id: part.device_identifier,
                            name: part.volume_name,
                            size: part.size,
                        })
                        .collect(),
                })
                .collect()
        }
        os => bail!("Unknown operating system: {os}"),
    };

    Ok(info.into_iter())
}

pub fn get_attached_disk_partitions(
    ssh_client: Option<&ssh::Client>,
) -> Result<impl Iterator<Item = DiskPartitionInfo>> {
    Ok(get_attached_disks(ssh_client)?.flat_map(|d| d.partitions))
}

#[doc_status]
#[define_commands(df)]
pub fn find_mount_dir(disk_id: &str) -> Result<PathBuf> {
    /// Find mount directory image disk
    {
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
    }
}

pub fn unnamed_if_empty<S: AsRef<str>>(name: S) -> String {
    if name.as_ref().trim().is_empty() {
        "<unnamed>".to_owned()
    } else {
        format!(r#""{}""#, name.as_ref())
    }
}

fn nullable_field<'de, D, T>(deserializer: D) -> StdResult<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    let opt = Option::<T>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_else(T::default))
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

#[derive(Debug, Clone)]
pub struct DiskInfo {
    pub id: String,
    pub part_type: String,
    pub name: String,
    pub size: usize,
    pub partitions: Vec<DiskPartitionInfo>,
}

impl Display for DiskInfo {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        self.description().fmt(f)
    }
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

#[derive(Debug, Clone)]
pub struct DiskPartitionInfo {
    pub id: String,
    pub size: usize,
    pub name: String,
}

impl Display for DiskPartitionInfo {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        self.description().fmt(f)
    }
}

impl DiskPartitionInfo {
    pub fn description(&self) -> String {
        format!(
            "{}: {} ({:.2} GB)",
            self.id,
            unnamed_if_empty(&self.name),
            self.size as f64 / 1e9,
        )
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DiskutilOutput {
    all_disks_and_partitions: Vec<DiskutilDisk>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DiskutilDisk {
    pub device_identifier: String,
    #[serde(default = "String::new")]
    pub volume_name: String,
    pub size: usize,
    pub content: String,
    #[serde(default = "Vec::new")]
    pub partitions: Vec<DiskutilPartition>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DiskutilPartition {
    pub device_identifier: String,
    #[serde(default = "String::new")]
    pub volume_name: String,
    pub size: usize,
}

#[derive(Deserialize)]
struct LsblkOutput {
    blockdevices: Vec<LsblkDisk>,
}

#[derive(Debug, Clone, Deserialize)]
struct LsblkDisk {
    name: String,
    #[serde(deserialize_with = "nullable_field")]
    fstype: String,
    kname: String,
    size: usize,
    #[serde(default = "Vec::new")]
    children: Vec<LsblkPartition>,
}

#[derive(Debug, Clone, Deserialize)]
struct LsblkPartition {
    name: String,
    label: String,
    size: usize,
}
