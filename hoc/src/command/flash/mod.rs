use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    mem,
    path::{Path, PathBuf},
};

use colored::Colorize;
use hocproc::procedure;
use tempfile::{NamedTempFile, TempDir};
use xz2::read::XzDecoder;
use zip::ZipArchive;

use hoclib::{cmd_macros, DirState};
use hoclog::{bail, choose, error, info, prompt, status, LogErr, Result};

use self::util::Image;

cmd_macros!(dd, diskutil, cmd_file => "file", hdiutil, sync);

mod util;

procedure! {
    pub struct Flash {
        #[procedure(rewind = DownloadOperatingSystemImage)]
        #[structopt(long)]
        redownload: bool,

        node_name: String,

        #[structopt(skip)]
        temp_file: Option<NamedTempFile>,
    }

    pub enum FlashState {
        DownloadOperatingSystemImage,

        DecompressZipArchive {
            archive_path: PathBuf,
            image: Image,
        },

        DecompressXzFile {
            file_path: PathBuf,
            image: Image,
        },

        ModifyRaspberryPiOsImage { image_path: PathBuf },

        #[procedure(transient)]
        ModifyUbuntuImage { image_path: PathBuf },

        #[procedure(finish)]
        FlashImage { image_path: PathBuf },
    }
}

impl Run for Flash {
    fn download_operating_system_image(
        &mut self,
        work_dir_state: &mut DirState,
    ) -> hoclog::Result<FlashState> {
        let images = Image::supported_versions();
        let default_index = images
            .iter()
            .position(|i| *i == Image::default())
            .unwrap_or_default();

        let index = choose!(
            "Which image do you want to use?",
            items = &images,
            default_index = default_index
        )?;

        let image = images[index];
        info!("URL: {}", image.url());

        let file_path = PathBuf::from("image");
        let file_real_path = status!("Download image" => {
            let file_real_path = work_dir_state.track(&file_path);
            let mut file = File::options()
                .read(false)
                .write(true)
                .create(true)
                .open(&file_real_path)?;

            reqwest::blocking::get(image.url()).log_err()?.copy_to(&mut file).log_err()?;
            file_real_path
        });

        let state = status!("Determine file type" => {
            let output = cmd_file!(file_real_path).run()?.1.to_lowercase();
            if output.contains("zip archive") {
                info!("Zip archive file type detected");
                DecompressZipArchive {
                    archive_path: file_path,
                    image,
                }
            } else if output.contains("xz compressed data") {
                info!("XZ compressed data file type detected");
                DecompressXzFile { file_path, image }
            } else {
                error!("Unsupported file type")?.into()
            }
        });

        Ok(state)
    }

    fn decompress_zip_archive(
        &mut self,
        work_dir_state: &mut DirState,
        archive_path: PathBuf,
        image: Image,
    ) -> Result<FlashState> {
        let (image_data, mut image_file) = status!("Read ZIP archive" => {
            let archive_real_path = work_dir_state.track(&archive_path);
            let file = File::options()
                .read(true)
                .write(true)
                .open(&archive_real_path)?;

            let mut archive = ZipArchive::new(&file).log_err()?;

            let mut buf = None;
            let archive_len = archive.len();
            for i in 0..archive_len {
                let mut archive_file = archive
                    .by_index(i)
                    .log_context("Failed to lookup image in ZIP archive")?;

                if archive_file.is_file() && archive_file.name().ends_with(".img") {
                    info!("Found image at index {} among {} items.", i, archive_len);

                    let mut data = Vec::new();
                    status!("Decompress image" => {
                        archive_file
                            .read_to_end(&mut data)
                            .log_context("Failed to read image in ZIP archive")?;
                        buf.replace(data);
                    });
                    break;
                }
            }

            if let Some(data) = buf {
                (data, file)
            } else {
                bail!("Image not found within ZIP archive");
            }
        });

        status!("Save decompressed image to file" => {
            image_file.seek(SeekFrom::Start(0))?;
            image_file.set_len(0)?;
            image_file.write_all(&image_data)?;
        });

        let state = match image {
            Image::RaspberryPiOs(_) => ModifyRaspberryPiOsImage {
                image_path: archive_path,
            },
            Image::Ubuntu(_) => ModifyUbuntuImage {
                image_path: archive_path,
            },
        };

        Ok(state)
    }

    fn decompress_xz_file(
        &mut self,
        work_dir_state: &mut DirState,
        file_path: PathBuf,
        image: Image,
    ) -> Result<FlashState> {
        let (image_data, mut image_file) = status!("Read XZ file" => {
            let file_real_path = work_dir_state.track(&file_path);
            let file = File::options()
                .read(true)
                .write(true)
                .open(&file_real_path)?;

            let mut decompressor = XzDecoder::new(&file);

            let mut buf = Vec::new();
            status!("Decompress image" => {
                decompressor
                    .read_to_end(&mut buf)
                    .log_context("Failed to read image in XZ file")?;
            });

            (buf, file)
        });

        status!("Save decompressed image to file" => {
            image_file.seek(SeekFrom::Start(0))?;
            image_file.set_len(0)?;
            image_file.write_all(&image_data)?;
        });

        let state = match image {
            Image::RaspberryPiOs(_) => ModifyRaspberryPiOsImage {
                image_path: file_path,
            },
            Image::Ubuntu(_) => ModifyUbuntuImage {
                image_path: file_path,
            },
        };

        Ok(state)
    }

    fn modify_raspberry_pi_os_image(
        &mut self,
        work_dir_state: &mut DirState,
        image_path: PathBuf,
    ) -> Result<FlashState> {
        let image_real_path = work_dir_state.track(&image_path);
        let (mount_dir, dev_disk_id) = self.attach_disk(&image_real_path, "boot")?;

        status!("Configure image" => {
            status!("Create SSH file"=> {
                File::create(mount_dir.as_ref().join("ssh"))?;
            });
        });

        self.detach_disk(dev_disk_id)?;

        Ok(FlashImage { image_path })
    }

    fn modify_ubuntu_image(
        &mut self,
        work_dir_state: &DirState,
        image_path: PathBuf,
    ) -> Result<FlashState> {
        let image_real_path = work_dir_state.get_path(&image_path)?;

        let image_temp_path = status!("Copy image to temporary location" => {
            let mut image_temp_file = NamedTempFile::new()?;
            let image_temp_path = image_temp_file.path().to_path_buf();

            info!("Destination: {}", image_temp_path.to_string_lossy());
            io::copy(&mut File::open(image_real_path)?, &mut image_temp_file)?;

            self.temp_file = Some(image_temp_file);
            image_temp_path
        });

        let (mount_dir, dev_disk_id) = self.attach_disk(&image_temp_path, "system-boot")?;

        status!("Configure image" => {
            use serde_yaml::Value;

            let mut user_data_path = mount_dir.path().to_path_buf();
            user_data_path.push("user-data");

            let mut map: BTreeMap<String, Value> =
                serde_yaml::from_reader(File::open(&user_data_path)?).log_err()?;

            let [heads @ .., last] = [
                ("hostname", Value::String(self.node_name.clone())),
                ("manage_etc_hosts", Value::Bool(true)),
            ];

            info!(
                "Updating {} with the following values:{}\n└╴{}: {}",
                "/user-data".blue(),
                heads
                    .iter()
                    .map(|(k, v)| Ok(format!("\n├╴{k}: {}", serde_json::to_string(v)?)))
                    .collect::<serde_json::Result<Vec<_>>>()
                    .log_err()?
                    .join(""),
                last.0,
                serde_json::to_string(&last.1).log_err()?,
            );

            for (k, mut v) in heads.into_iter().chain([last].into_iter()) {
                map.entry(k.to_string())
                    .and_modify(|e| *e = mem::take(&mut v))
                    .or_insert_with(|| v);
            }

            serde_yaml::to_writer(
                File::options()
                    .write(true)
                    .truncate(true)
                    .open(&user_data_path)?,
                &map,
            )
            .log_err()?;
        });

        self.detach_disk(dev_disk_id)?;

        Ok(FlashImage {
            image_path: image_temp_path,
        })
    }

    fn flash_image(&mut self, work_dir_state: &mut DirState, image_path: PathBuf) -> Result<()> {
        let disk_id = status!("Find mounted SD card" => {
            let mut physical_disk_infos: Vec<_> =
                util::get_attached_disks([util::DiskType::Physical])
                    .log_context("Failed to get attached disks")?;

            let index = choose!("Choose which disk to flash", items = &physical_disk_infos)?;
            physical_disk_infos.remove(index).id
        });

        let disk_path = PathBuf::from(format!("/dev/{}", disk_id));

        status!("Unmount SD card" => diskutil!("unmountDisk", disk_path).run()?);

        let image_real_path = work_dir_state.track(&image_path);

        status!("Flash SD card" => {
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

        status!("Unmount image disk" => {
            diskutil!("unmountDisk", disk_path).run()?
        });

        Ok(())
    }
}

impl Flash {
    fn attach_disk(&self, image_path: &Path, partition_name: &str) -> Result<(TempDir, String)> {
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
                util::get_attached_disks([util::DiskType::Virtual])
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
            let mount_dir = TempDir::new().log_err()?;
            diskutil!("mount", "-mountPoint", mount_dir.as_ref(), &dev_disk_id).run()?;
            mount_dir
        });

        Ok((mount_dir, dev_disk_id))
    }

    fn detach_disk(&self, dev_disk_id: String) -> Result<()> {
        status!("Sync image disk writes" => sync!().run()?);
        status!("Unmount image disk" => {
            diskutil!("unmountDisk", dev_disk_id).run()?
        });
        status!("Detach image disk" => {
            hdiutil!("detach", dev_disk_id).run()?
        });

        Ok(())
    }
}
