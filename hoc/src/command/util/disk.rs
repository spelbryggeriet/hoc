use std::{
    fmt::{self, Display, Formatter},
    path::{Path, PathBuf},
};

use hoclib::DirState;
use hoclog::{choose, status, LogErr, Result};
use serde::Deserialize;

pub fn get_attached_disks<I: IntoIterator<Item = Type>>(
    disk_types: I,
) -> Result<Vec<AttachedDiskInfo>> {
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

pub fn attach_disk(image_path: &Path, partition_name: &str) -> Result<(PathBuf, String)> {
    status!("Attach image as disk" => {
        hdiutil!(
            "attach",
            "-imagekey",
            "diskimage-class=CRawDiskImage",
            "-nomount",
            image_path
        )
        .run()?
    });

    let disk_id = status!("Find attached disk" => {
        let mut attached_disks_info: Vec<_> =
            get_attached_disks([Type::Virtual])
                .log_context("Failed to get attached disks")?
                .into_iter()
                .filter(|adi| adi.partitions.iter().any(|p| p.name == partition_name))
                .collect();

        let index = choose!(
            "Which disk do you want to use?",
            items = &attached_disks_info,
        )?;

        attached_disks_info
            .remove(index)
            .partitions
            .into_iter()
            .find(|p| p.name == partition_name)
            .unwrap()
            .id
    });

    let dev_disk_id = format!("/dev/{}", disk_id);

    let mount_dir = status!("Mount image disk" => {
        let mount_dir = DirState::create_temp_dir("mounted_image")?;
        diskutil!("mount", "-mountPoint", mount_dir, dev_disk_id).run()?;
        mount_dir
    });

    Ok((mount_dir, dev_disk_id))
}

pub fn detach_disk(dev_disk_id: String) -> Result<()> {
    status!("Sync image disk writes" => sync!().run()?);
    status!("Unmount image disk" => {
        diskutil!("unmountDisk", dev_disk_id).run()?
    });
    status!("Detach image disk" => {
        hdiutil!("detach", dev_disk_id).run()?
    });

    Ok(())
}

pub fn unnamed_if_empty<S: AsRef<str>>(name: S) -> String {
    if name.as_ref().trim().is_empty() {
        "<unnamed>".to_owned()
    } else {
        format!(r#""{}""#, name.as_ref())
    }
}

pub const fn type_physical() -> Type {
    Type::Physical
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Type {
    Physical,
    Virtual,
}

impl AsRef<str> for Type {
    fn as_ref(&self) -> &str {
        match self {
            Self::Physical => "physical",
            Self::Virtual => "virtual",
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

    #[serde(skip_deserializing, default = "type_physical")]
    pub disk_type: Type,

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
