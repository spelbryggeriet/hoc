use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

use hocproc::procedure;
use strum::IntoEnumIterator;
use tempfile::TempDir;
use zip::ZipArchive;

use hoclib::{cmd_macros, halt, transient_finish, DirState, Halt};
use hoclog::{bail, choose, info, prompt, status, LogErr};

cmd_macros!(dd, diskutil, hdiutil, sync);

mod util;

procedure! {
    pub struct Flash {
        #[procedure(rewind = DownloadOperatingSystemImage)]
        #[structopt(long)]
        redownload: bool,
    }

    pub enum FlashState {
        DownloadOperatingSystemImage,
        DecompressImageArchive { archive_path: PathBuf },
        ModifyImage { image_path: PathBuf },
        FlashImage { image_path: PathBuf },
    }
}

impl Steps for Flash {
    fn download_operating_system_image(
        &mut self,
        work_dir_state: &mut DirState,
    ) -> hoclog::Result<Halt<FlashState>> {
        let mut images: Vec<_> = util::Image::iter().collect();
        let index = choose!("Which image do you want to use?", items = &images)?;

        let image = images.remove(index);
        info!("Image: {}", image);
        info!("URL  : {}", image.url());

        let archive_path = PathBuf::from("image");
        status!("Downloading image" => {
            let image_real_path = work_dir_state.track(&archive_path);
            let mut file = File::options()
                .read(false)
                .write(true)
                .create(true)
                .open(image_real_path)?;

            reqwest::blocking::get(image.url()).log_err()?.copy_to(&mut file).log_err()?;
        });

        halt!(DecompressImageArchive { archive_path })
    }

    fn decompress_image_archive(
        &mut self,
        work_dir_state: &mut DirState,
        archive_path: PathBuf,
    ) -> hoclog::Result<Halt<FlashState>> {
        let (archive_data, mut archive_file) = status!("Reading archive" => {
            let archive_real_path = work_dir_state.track(&archive_path);
            let mut file = File::options()
                .read(true)
                .write(true)
                .open(&archive_real_path)?;

            let mut archive = ZipArchive::new(&file).log_err()?;

            let mut buf = None;
            let archive_len = archive.len();
            for i in 0..archive_len {
                let mut archive_file = archive
                    .by_index(i)
                    .log_context("Failed to lookup image in Zip archive")?;

                if archive_file.is_file() && archive_file.name().ends_with(".img") {
                    info!("Found image at index {} among {} items.", i, archive_len);

                    let mut data = Vec::new();
                    status!("Decompressing image" => {
                        archive_file
                            .read_to_end(&mut data)
                            .log_context("Failed to read image in Zip archive")?;
                        buf.replace(data);
                    });
                    break;
                }
            }

            file.seek(SeekFrom::Start(0))?;
            file.set_len(0)?;

            if let Some(data) = buf {
                (data, file)
            } else {
                bail!("Image not found within Zip archive");
            }
        });

        status!("Save decompressed image to file" => {
            archive_file.write_all(&archive_data)?;
        });

        halt!(ModifyImage {
            image_path: archive_path,
        })
    }

    fn modify_image(
        &mut self,
        work_dir_state: &mut DirState,
        image_path: PathBuf,
    ) -> hoclog::Result<Halt<FlashState>> {
        let image_real_path = work_dir_state.track(&image_path);

        status!("Attaching image as disk" => {
            hdiutil!(
                "attach",
                "-imagekey",
                "diskimage-class=CRawDiskImage",
                "-nomount",
                image_real_path
            )
            .run()?
        });

        let disk_id = status!("Find attached disk" => {
            let mut attached_disks_info: Vec<_> =
                util::get_attached_disks([util::DiskType::Virtual])
                    .log_context("Failed to get attached disks")?
                    .into_iter()
                    .filter(|adi| adi.partitions.iter().any(|p| p.name == "boot"))
                    .collect();

            let index = choose!(
                "Which disk do you want to use?",
                items = &attached_disks_info,
            )?;

            attached_disks_info
                .remove(index)
                .partitions
                .into_iter()
                .find(|p| p.name == "boot")
                .unwrap()
                .id
        });

        let dev_disk_id = format!("/dev/{}", disk_id);

        let mount_dir = status!("Mounting image disk" => {
            let mount_dir = TempDir::new().log_err()?;
            diskutil!("mount", "-mountPoint", mount_dir.as_ref(), &dev_disk_id).run()?;
            mount_dir
        });

        status!("Configure image" => {
            status!("Creating SSH file"=> {
                File::create(mount_dir.as_ref().join("ssh"))?;
            });
        });

        status!("Syncing image disk writes" => sync!().run()?);
        status!("Unmounting image disk" => {
            diskutil!("unmountDisk", dev_disk_id).run()?
        });
        status!("Detaching image disk" => {
            hdiutil!("detach", dev_disk_id).run()?
        });

        halt!(FlashImage { image_path })
    }

    fn flash_image(
        &mut self,
        work_dir_state: &mut DirState,
        image_path: PathBuf,
    ) -> hoclog::Result<Halt<FlashState>> {
        let disk_id = status!("Find mounted SD card" => {
            let mut physical_disk_infos: Vec<_> =
                util::get_attached_disks([util::DiskType::Physical])
                    .log_context("Failed to get attached disks")?;

            let index = choose!("Choose which disk to flash", items = &physical_disk_infos)?;
            physical_disk_infos.remove(index).id
        });

        let disk_path = PathBuf::from(format!("/dev/{}", disk_id));

        status!("Unmounting SD card" => diskutil!("unmountDisk", disk_path).run()?);

        let image_real_path = work_dir_state.track(&image_path);

        status!("Flashing SD card" => {
            prompt!("Do you want to flash target disk '{}'?", disk_id)?;

            dd!(
                "bs=1m",
                format!("if={}", image_real_path.to_string_lossy()),
                format!("of=/dev/r{disk_id}"),
            )
            .sudo()
            .run()?;

            info!(
                "Image '{}' flashed to target disk '{}'",
                image_path.to_string_lossy(),
                disk_id,
            );
        });

        status!("Unmounting image disk" => {
            diskutil!("unmountDisk", disk_path).run()?
        });

        transient_finish!()
    }
}
