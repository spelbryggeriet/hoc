use std::fs::File;

use tempfile::TempDir;

use super::*;

impl Flash {
    pub(super) fn modify(
        &self,
        step: &mut ProcedureStep,
        image_path: PathBuf,
    ) -> hoclog::Result<Halt<FlashState>> {
        let image_real_path = step.register_path(&image_path).log_err()?;

        status!(
            "Attaching image as disk",
            cmd!(
                "hdiutil",
                "attach",
                "-imagekey",
                "diskimage-class=CRawDiskImage",
                "-nomount",
                image_real_path,
            )?,
        );

        let disk_id = status!("Find attached disk", {
            let mut attached_disks_info: Vec<_> =
                util::get_attached_disks([util::DiskType::Virtual])
                    .log_context("Failed to get attached disks")?
                    .into_iter()
                    .filter(|adi| adi.partitions.iter().any(|p| p.name == "boot"))
                    .collect();

            let boot_disk_descs = attached_disks_info
                .iter()
                .map(util::AttachedDiskInfo::description);

            let index =
                choose!("Which disk do you want to use?", items = boot_disk_descs).log_err()?;

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
            )?;
            mount_dir
        });

        status!("Configure image", {
            status!("Creating SSH file", {
                File::create(mount_dir.as_ref().join("ssh")).log_err()?;
            });
        });

        status!("Syncing image disk writes", cmd!("sync")?);

        status!(
            "Unmounting image disk",
            cmd!("diskutil", "unmountDisk", &dev_disk_id)?,
        );

        status!(
            "Detaching image disk",
            cmd!("hdiutil", "detach", dev_disk_id)?,
        );

        Ok(Halt::Yield(FlashState::Flash { image_path }))
    }
}
