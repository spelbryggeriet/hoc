use super::*;

impl Flash {
    pub(super) fn flash(
        &self,
        step: &mut ProcedureStep,
        image_path: PathBuf,
    ) -> Result<Halt<FlashState>> {
        let disk_id = status!("Find mounted SD card" => {
            let mut physical_disk_infos: Vec<_> =
                util::get_attached_disks([util::DiskType::Physical])
                    .log_context("Failed to get attached disks")?;

            let index =
                choose!("Choose which disk to flash", items = &physical_disk_infos).log_err()?;
            physical_disk_infos.remove(index).id
        });

        let disk_path = PathBuf::from(format!("/dev/{}", disk_id));

        status!("Unmounting SD card" => cmd!("diskutil", "unmountDisk", disk_path).run()?);

        let image_real_path = step.register_file(&image_path)?;

        status!("Flashing SD card" => {
            prompt!("Do you want to flash target disk '{}'?", disk_id).log_err()?;

            cmd!(
                "dd",
                "bs=1m",
                format!("if={}", image_real_path.to_string_lossy()),
                format!("of=/dev/r{}", disk_id),
            )
            .sudo()
            .run()?;

            info!(
                "Image '{}' flashed to target disk '{}'",
                image_path.to_string_lossy(),
                disk_id,
            );
        });

        status!("Unmounting image disk" => cmd!("diskutil", "unmountDisk", disk_path).run()?);

        Ok(Halt::transient_finish())
    }
}
