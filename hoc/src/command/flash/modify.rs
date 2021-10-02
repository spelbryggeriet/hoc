use std::fs::File;

use serde::Deserialize;
use tempfile::TempDir;

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
        let image_real_path = proc_step.register_path(&image_path).log_err()?;

        status!(
            "Attaching image as disk",
            cmd!(
                "hdiutil",
                "attach",
                "-imagekey",
                "diskimage-class=CRawDiskImage",
                "-nomount",
                image_real_path,
            ),
        );

        let disk_id = status!("Searching for the correct disk", {
            let stdout = cmd_silent!("diskutil", "list", "-plist", "external", "virtual");

            let mut attached_disks_info: Vec<_> = plist::from_bytes::<DiskutilOutput>(&stdout)
                .log_context("Failed to parse output of 'diskutil'")?
                .all_disks_and_partitions
                .into_iter()
                .filter(|adi| adi.partitions.iter().any(|p| p.name == "boot"))
                .collect();

            let boot_disk_descs = attached_disks_info.iter().map(|adi| {
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

            let index = choose!("Which disk do you want to use?", items = boot_disk_descs)
                .log_context("Failed to present list of disks to choose from")?;

            attached_disks_info
                .remove(index)
                .partitions
                .into_iter()
                .find(|p| p.name == "boot")
                .unwrap()
                .id
        });

        let dev_disk_id = format!("/dev/{}", disk_id);

        let mount_dir = status!("Mounting image disk", {
            let mount_dir = TempDir::new().log_err()?;
            cmd!(
                "diskutil",
                "mount",
                "-mountPoint",
                mount_dir.as_ref(),
                &dev_disk_id,
            );
            mount_dir
        });

        status!("Configure image", {
            status!("Creating SSH file", {
                File::create(mount_dir.as_ref().join("ssh")).log_err()?;
            });
        });

        status!("Syncing image disk writes", cmd!("sync"));

        status!(
            "Unmounting image disk",
            cmd!("diskutil", "unmountDisk", &dev_disk_id)
        );

        status!(
            "Unmounting image disk",
            cmd!("hdiutil", "detach", dev_disk_id)
        );

        Ok(Halt::Finish)
    }
}
