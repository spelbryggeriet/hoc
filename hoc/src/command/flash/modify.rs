use std::process::Command;

use serde::Deserialize;

use super::*;

fn unamed_if_empty<S: AsRef<str>>(name: S) -> String {
    if name.as_ref().trim().is_empty() {
        "<unnamed>".to_owned()
    } else {
        format!(r#""{}""#, name.as_ref())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DiskutilOutput {
    all_disks_and_partitions: Vec<AttachedDiskInfo>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AttachedDiskInfo {
    #[serde(rename = "DeviceIdentifier")]
    id: String,

    #[serde(rename = "Content")]
    part_type: String,

    #[serde(rename = "VolumeName", default = "String::new")]
    name: String,

    size: usize,

    #[serde(default = "Vec::new")]
    partitions: Vec<AttachedDiskPartitionInfo>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AttachedDiskPartitionInfo {
    #[serde(rename = "DeviceIdentifier")]
    id: String,

    #[serde(rename = "Content", default = "String::new")]
    part_type: String,

    size: usize,

    #[serde(rename = "VolumeName", default = "String::new")]
    name: String,
}

impl Flash {
    pub(super) fn modify(
        &self,
        proc_step: &mut ProcedureStep,
        image_path: PathBuf,
    ) -> hoclog::Result<Halt<FlashState>> {
        status!("Attaching disk" => {
            run_system_command(
                Command::new("hdiutil")
                    .args(&[
                        "attach",
                        "-imagekey",
                        "diskimage-class=CRawDiskImage",
                        "-nomount",
                    ])
                    .arg(proc_step.file_path(image_path).log_err()?),
            )?;
        });

        status!("Searching for the correct disk" => {
            let stdout = Command::new("diskutil")
                .args(&["list", "-plist", "external", "virtual"])
                .output()
                .log_context("Failed running command 'diskutil'")?
                .stdout;

            let attached_disks_info = plist::from_bytes::<DiskutilOutput>(&stdout)
                .log_context("Failed to parse output of 'diskutil'")?
                .all_disks_and_partitions;

            let boot_disk_descs = attached_disks_info
                .iter()
                .filter(|adi| adi.partitions.iter().any(|p| p.name == "boot"))
                .map(|adi| {
                    let mut desc = unamed_if_empty(&adi.name);
                    if !adi.partitions.is_empty() {
                        desc += &format!(
                            " ({} partition{}: {})",
                            adi.partitions.len(),
                            if adi.partitions.len() == 1 { "" } else { "s" },
                            adi.partitions
                                .iter()
                                .map(|p| unamed_if_empty(&p.name))
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                    }
                    desc + &format!(", {:.2} GB", adi.size as f64 / 1e9)
                });

            choose!("Which disk do you want to use?", items = boot_disk_descs)
                .log_context("Failed to present list of disks to choose from")?;
        });

        Ok(Halt::Finish)
    }
}
