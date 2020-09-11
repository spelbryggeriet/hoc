mod parse;

use std::collections::HashMap as Map;
use std::convert::TryFrom;
use std::ffi::OsString;
use std::fmt::{self, Display, Formatter};
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context};
use heck::SnakeCase;
use num_enum::TryFromPrimitive;
use serde::Deserialize;
use structopt::StructOpt;
use strum::IntoEnumIterator;
use xz2::read::XzDecoder;
use zip::ZipArchive;

use crate::prelude::*;

#[derive(Clone, Copy, Eq, PartialEq, TryFromPrimitive)]
#[repr(isize)]
enum FlashCacheState {
    Initial,
    Downloaded,
    Decompressed,
    Modified,
}

impl Default for FlashCacheState {
    fn default() -> Self {
        Self::Initial
    }
}

#[derive(StructOpt)]
pub(super) struct CmdFlash {}

impl CmdFlash {
    pub(super) async fn run(self, context: &mut AppContext, log: &mut Logger) -> AppResult<()> {
        let image = self.select_image(log).context("Selecting image")?;

        let image_key = image.description().to_snake_case();

        let mut image_size = None;
        loop {
            let (current_state, temp_file) = context.start_cache_writing(&image_key)?;

            let state = match FlashCacheState::try_from(current_state).unwrap_or_default() {
                FlashCacheState::Initial => {
                    self.fetch_image(log, image, temp_file.as_file_mut())
                        .await
                        .with_context(|| format!("Fetching image '{}'", image))?;

                    FlashCacheState::Downloaded
                }

                FlashCacheState::Downloaded => {
                    let image_size_value = self
                        .decompress_image(log, image, temp_file.as_file_mut())
                        .context("Decompressing image")?;
                    image_size.replace(image_size_value);

                    FlashCacheState::Decompressed
                }

                FlashCacheState::Decompressed => {
                    self.attach_disk(log, temp_file.path())
                        .context("Attaching image disk")?;

                    let attached_disk_info =
                        self.get_attahed_disks().context("Getting attached disks")?;

                    let boot_disk_id = attached_disk_info
                        .into_iter()
                        .filter(|i| i.disk_type == "virtual")
                        .flat_map(|i| i.partitions)
                        .find_map(|p| if p.name == "boot" { Some(p.id) } else { None })
                        .context("Could not find 'boot' partition")?;

                    let mount_dir =
                        TempDir::new().context("Creating temporary mounting directory")?;
                    self.mount_disk(log, &boot_disk_id, mount_dir.path())
                        .context("Mounting image disk")?;

                    println!("{}", mount_dir.as_ref().to_string_lossy());

                    log.prompt("Continue?")?;

                    log.status("Creating ssh file")?;
                    File::create(mount_dir.as_ref().join("ssh")).context("Creating ssh file")?;

                    log.prompt("Continue?")?;

                    self.unmount_disk(log, &boot_disk_id)
                        .context("Unmounting image disk")?;

                    log.prompt("Continue?")?;

                    self.detach_disk(log, &boot_disk_id)
                        .context("Detaching image disk")?;

                    FlashCacheState::Modified
                }

                FlashCacheState::Modified => FlashCacheState::Modified,
            };

            // Stop cache writing, to write state change to file.
            context.stop_cache_writing(&image_key, state as isize)?;

            // Peform post cache writing operations, if any.
            match state {
                FlashCacheState::Initial => (),
                FlashCacheState::Downloaded => (),
                FlashCacheState::Decompressed => {
                    let image_file = context.get_named_file(&image_key)?;

                    match image {
                        Image::Fedora => {
                            let _image_size =
                                image_size.map(|v| AppResult::Ok(v)).unwrap_or_else(|| {
                                    Ok(image_file.as_file().metadata()?.len() as usize)
                                })?;

                            let mut image_info = self
                                .get_image_info(image_file.path())
                                .context("Getting image information")?;

                            let largest_partition_index = image_info
                                .iter()
                                .enumerate()
                                .max_by_key(|(_, p)| p.size)
                                .map(|(i, _)| i)
                                .unwrap_or_default();

                            let index = log.choose(
                                    "Choose the partition where the OS lives (most likely the largest one)",
                                    image_info.iter(),
                                    largest_partition_index,
                                )?;

                            let image_partition_info = image_info.remove(index);
                        }

                        Image::Raspbian => (),
                    }
                }
                FlashCacheState::Modified => break,
            }
        }

        let image_file = context.get_named_file(&image_key)?;

        let mut physical_disks_info: Vec<_> = self
            .get_attahed_disks()?
            .into_iter()
            .filter(|d| d.disk_type == "physical")
            .collect();

        if physical_disks_info.is_empty() {
            anyhow::bail!("No external physical disks mounted");
        }

        let index = log.choose("Choose which disk to flash", physical_disks_info.iter(), 0)?;
        let attached_disk_info = physical_disks_info.remove(index);

        let attached_disk_info_str = attached_disk_info.to_string();
        log.prompt(format!(
            "Do you want to flash target disk '{}'?",
            attached_disk_info_str
        ))?;

        self.unmount_disk(log, &attached_disk_info.id)
            .context("Unmounting attached disk")?;

        self.flash_target_disk(log, attached_disk_info, image_file.path())
            .with_context(|| format!("Flashing target disk '{}'", attached_disk_info_str))?;

        Ok(())
    }

    fn select_image(&self, log: &mut Logger) -> AppResult<Image> {
        let index = log.choose(
            "Choose which operating system image to download",
            Image::iter().map(|i| i.description()),
            0,
        )?;

        Ok(Image::iter().nth(index).unwrap())
    }

    async fn fetch_image<'a: 'b, 'b>(
        &'a self,
        log: &'b mut Logger,
        image: Image,
        dest: &mut File,
    ) -> AppResult<()> {
        log.status(format!("Fetching image '{}' from '{}'", image, image.url()))?;
        let bytes = reqwest::get(image.url()).await?.bytes().await?;

        log.status(format!("Writing image '{}' to file", image))?;
        dest.write(bytes.as_ref())
            .with_context(|| format!("Writing image '{}' to file", image))?;

        Ok(())
    }

    fn decompress_image(
        &self,
        log: &mut Logger,
        image: Image,
        file: &mut File,
    ) -> AppResult<usize> {
        let size = match image.compression_type() {
            CompressionType::Xz => self.decompress_xz_image(log, file)?,
            CompressionType::Zip => self.decompress_zip_image(log, file)?,
        };

        Ok(size)
    }

    fn decompress_xz_image(&self, log: &mut Logger, file: &mut File) -> AppResult<usize> {
        let mut decompressor = XzDecoder::new(&*file);

        log.status("Decompressing image")?;
        let mut buf = Vec::new();
        decompressor
            .read_to_end(&mut buf)
            .context("Reading decompressed image file")?;

        file.seek(SeekFrom::Start(0))?;
        file.write(&buf)
            .context("Writing decompressed content back to file")
    }

    fn decompress_zip_image(&self, log: &mut Logger, file: &mut File) -> AppResult<usize> {
        let mut archive = ZipArchive::new(&*file).context("Reading Zip file")?;

        for i in 0..archive.len() {
            let mut archive_file = archive.by_index(i).context("Reading archive file")?;

            if archive_file.is_file() && archive_file.name().ends_with(".img") {
                log.status("Decompressing image")?;
                let mut buf = Vec::new();
                archive_file
                    .read_to_end(&mut buf)
                    .context("Reading decompressed image file")?;
                drop(archive_file);

                file.seek(SeekFrom::Start(0))?;
                return file
                    .write(&buf)
                    .context("Writing decompressed content back to file");
            }
        }

        Err(anyhow!("Image not found within Zip archive"))
    }

    fn get_image_info(&self, image_path: impl AsRef<Path>) -> AppResult<Vec<ImagePartitionInfo>> {
        if cfg!(target_os = "macos") {
            let stdout = Command::new("hdiutil")
                .args(&["imageinfo", "-plist"])
                .arg(image_path.as_ref())
                .output()
                .context("Executing hdiutil")?
                .stdout;

            #[derive(Deserialize)]
            struct HdiUtilOutput {
                partitions: HdiUtilPartitionsInfo,
            }

            #[derive(Deserialize)]
            #[serde(rename_all = "kebab-case")]
            struct HdiUtilPartitionsInfo {
                block_size: usize,
                partitions: Vec<HdiUtilPartitionInfo>,
            }

            #[derive(Deserialize)]
            #[serde(rename_all = "kebab-case")]
            struct HdiUtilPartitionInfo {
                partition_hint: String,
                partition_length: usize,
                partition_filesystems: Map<String, String>,
            }

            let output: HdiUtilOutput =
                plist::from_bytes(&stdout).context("Parsing hdiutil output")?;

            let mut image_info = Vec::new();
            for hdiutil_partition_info in output.partitions.partitions {
                image_info.push(ImagePartitionInfo {
                    name: hdiutil_partition_info
                        .partition_filesystems
                        .into_iter()
                        .next()
                        .map(|(_, v)| v)
                        .unwrap_or_default(),
                    part_type: hdiutil_partition_info.partition_hint,
                    size: hdiutil_partition_info.partition_length * output.partitions.block_size,
                })
            }

            Ok(image_info)
        } else if cfg!(target_os = "linux") {
            let stdout = Command::new("fdisk")
                .arg("-l")
                .args(&["-o", "Start,Sectors,Type"])
                .arg(image_path.as_ref())
                .output()
                .context("Executing fdisk")?
                .stdout;

            let output = String::from_utf8(stdout).context("Converting stdout to UTF-8")?;
            let (_, mut disk_info) = parse::source_disk_info(&output)
                .map_err(|e| anyhow!(e.to_string()))
                .context("Parsing disk info")?;

            todo!("Use intermediate format here instead")
        } else {
            anyhow::bail!("Windows not supported");
        }
    }

    fn attach_disk(&self, log: &mut Logger, image_path: impl AsRef<Path>) -> AppResult<()> {
        log.status("Attaching disk")?;

        let output = if cfg!(target_os = "macos") {
            Command::new("hdiutil")
                .args(&[
                    "attach",
                    "-imagekey",
                    "diskimage-class=CRawDiskImage",
                    "-nomount",
                ])
                .arg(image_path.as_ref())
                .output()
                .context("Executing hdiutil")?
        } else if cfg!(target_os = "linux") {
            unimplemented!()
        } else {
            anyhow::bail!("Windows not supported");
        };

        anyhow::ensure!(
            output.status.success(),
            format!("{}", String::from_utf8_lossy(&output.stderr))
        );

        Ok(())
    }

    fn mount_disk(
        &self,
        log: &mut Logger,
        disk_id: impl AsRef<str>,
        mount_dir: impl AsRef<Path>,
    ) -> AppResult<()> {
        log.status("Mounting disk")?;

        let output = if cfg!(target_os = "macos") {
            Command::new("diskutil")
                .args(&["mount", "-mountPoint"])
                .arg(mount_dir.as_ref())
                .arg(format!("/dev/{}", disk_id.as_ref()))
                .output()
                .context("Executing diskutil")?
        } else if cfg!(target_os = "linux") {
            unimplemented!()
        } else {
            anyhow::bail!("Windows not supported");
        };

        anyhow::ensure!(
            output.status.success(),
            format!("{}", String::from_utf8_lossy(&output.stderr))
        );

        Ok(())
    }

    fn unmount_disk(&self, log: &mut Logger, disk_id: impl AsRef<str>) -> AppResult<()> {
        log.status("Unmounting disk")?;

        let output = if cfg!(target_os = "macos") {
            Command::new("diskutil")
                .arg("unmountDisk")
                .arg(format!("/dev/{}", disk_id.as_ref()))
                .output()
                .context("Executing diskutil")?
        } else if cfg!(target_os = "linux") {
            unimplemented!();
        } else {
            anyhow::bail!("Windows not supported");
        };

        anyhow::ensure!(
            output.status.success(),
            String::from_utf8_lossy(&output.stderr).into_owned()
        );

        Ok(())
    }

    fn detach_disk(&self, log: &mut Logger, disk_id: impl AsRef<str>) -> AppResult<()> {
        log.status("Detaching disk")?;

        let output = if cfg!(target_os = "macos") {
            Command::new("hdiutil")
                .arg("detach")
                .arg(format!("/dev/{}", disk_id.as_ref()))
                .output()
                .context("Executing hdiutil")?
        } else if cfg!(target_os = "linux") {
            unimplemented!()
        } else {
            anyhow::bail!("Windows not supported");
        };

        anyhow::ensure!(
            output.status.success(),
            format!("{}", String::from_utf8_lossy(&output.stderr))
        );

        Ok(())
    }

    fn get_attahed_disks(&self) -> AppResult<Vec<AttachedDiskInfo>> {
        let attached_disks_info = if cfg!(target_os = "macos") {
            #[derive(Deserialize)]
            #[serde(rename_all = "PascalCase")]
            struct DiskutilOutput {
                all_disks_and_partitions: Vec<AttachedDiskInfo>,
            }

            let mut attached_disks_info = Vec::new();
            for disk_type in &["physical", "virtual"] {
                let stdout = Command::new("diskutil")
                    .args(&["list", "-plist", "external", disk_type])
                    .output()
                    .context("Executing diskutil")?
                    .stdout;

                let output: DiskutilOutput =
                    plist::from_bytes(&stdout).context("Parsing diskutil output")?;

                attached_disks_info.extend(output.all_disks_and_partitions.into_iter().map(
                    |mut d| {
                        d.disk_type = disk_type.to_string();
                        d
                    },
                ))
            }

            attached_disks_info
        } else if cfg!(target_os = "linux") {
            let stdout = Command::new("lsblk")
                .arg("-bOJ")
                .output()
                .context("Executing lsblk")?
                .stdout;

            #[derive(Deserialize)]
            struct LsblkOutput {
                blockdevices: Vec<AttachedDiskInfo>,
            }

            let mut output: LsblkOutput =
                serde_json::from_reader(Cursor::new(stdout)).context("Parsing lsblk output")?;

            output.blockdevices = output
                .blockdevices
                .into_iter()
                .filter(|bd| bd.id.starts_with("sd"))
                .collect();

            output.blockdevices
        } else {
            anyhow::bail!("Windows not supported");
        };

        if attached_disks_info.is_empty() {
            anyhow::bail!("No external disks mounted");
        }

        Ok(attached_disks_info)
    }

    fn flash_target_disk(
        &self,
        log: &mut Logger,
        disk_info: AttachedDiskInfo,
        image_path: impl AsRef<Path>,
    ) -> AppResult<()> {
        let disk_info_str = disk_info.to_string();

        log.status(format!("Flashing disk '{}'", disk_info_str))?;
        let output = if cfg!(target_family = "unix") {
            let mut arg_if = OsString::from("if=");
            arg_if.push(image_path.as_ref());

            Command::new("dd")
                .arg("bs=1m")
                .arg(arg_if)
                .arg(format!(
                    "of=/dev/{}{}",
                    if cfg!(target_os = "macos") { "r" } else { "" },
                    disk_info.id
                ))
                .output()
                .context("Executing dd")?
        } else {
            anyhow::bail!("Windows not supported");
        };

        anyhow::ensure!(
            output.status.success(),
            format!("{}", String::from_utf8_lossy(&output.stderr))
        );

        log.info(format!(
            "Image '{}' flashed to target disk '{}'",
            image_path.as_ref().to_string_lossy(),
            disk_info_str
        ))?;
        Ok(())
    }
}

#[derive(Clone, Copy, EnumIter, Eq, PartialEq)]
enum Image {
    Fedora,
    Raspbian,
}

impl Image {
    pub const fn description(&self) -> &'static str {
        match self {
            Self::Fedora => "Fedora 32",
            Self::Raspbian => "Raspbian Latest",
        }
    }

    pub const fn url(&self) -> &'static str {
        match self {
            Self::Fedora => "https://download.fedoraproject.org/pub/fedora/linux/releases/32/Server/armhfp/images/Fedora-Server-armhfp-32-1.6-sda.raw.xz",
            Self::Raspbian => "https://downloads.raspberrypi.org/raspios_lite_armhf_latest",
        }
    }

    pub const fn compression_type(&self) -> CompressionType {
        match self {
            Self::Fedora => CompressionType::Xz,
            Self::Raspbian => CompressionType::Zip,
        }
    }
}

impl Display for Image {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.description().fmt(f)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum CompressionType {
    Xz,
    Zip,
}
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AttachedDiskInfo {
    #[serde(rename = "DeviceIdentifier")]
    id: String,

    #[serde(rename = "Content")]
    part_type: String,

    #[serde(default = "String::new")]
    disk_type: String,

    #[serde(rename = "VolumeName", default = "String::new")]
    name: String,

    size: usize,

    #[serde(default = "Vec::new")]
    partitions: Vec<AttachedDiskPartitionInfo>,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Deserialize)]
struct AttachedDiskInfo {
    #[serde(rename = "name")]
    id: String,

    #[serde(rename = "type")]
    part_type: String,

    #[serde(rename = "kname", default = "String::new")]
    name: String,

    size: usize,

    #[serde(default = "Vec::new", rename = "children")]
    partitions: Vec<AttachedDiskPartitionInfo>,
}

impl Display for AttachedDiskInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let (size, unit) = crate::readable_size(self.size);

        write!(
            f,
            "id: {:>7} | type: {:>30} | size: {:>5.1} {}",
            self.id, self.part_type, size, unit
        )?;
        if self.partitions.len() > 0 {
            write!(f, "\n  {}", "-".repeat(67))?;

            for partition in &self.partitions {
                write!(f, "\n  {}", partition)?;
            }
        }

        Ok(())
    }
}

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Deserialize)]
struct AttachedDiskPartitionInfo {
    #[serde(rename = "name")]
    id: String,

    #[serde(rename = "type", default = "String::new")]
    part_type: String,

    size: usize,

    #[serde(rename = "label", default = "String::new")]
    name: String,
}

impl Display for AttachedDiskPartitionInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let (size, unit) = crate::readable_size(self.size);

        write!(
            f,
            "id: {:>7} | type: {:>30} | size: {:>5.1} {} | name: {:>10}",
            self.id, self.part_type, size, unit, self.name,
        )?;

        Ok(())
    }
}

#[derive(Debug, Clone)]
struct SourceDiskInfo {
    num_sectors: usize,
    partitions: Vec<SourceDiskPartitionInfo>,
}

struct ImagePartitionInfo {
    name: String,
    part_type: String,
    size: usize,
}

impl Display for ImagePartitionInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let (size, unit) = crate::readable_size(self.size);

        write!(
            f,
            "name: {:>15} | type: {:>30} | size: {:>5.1} {}",
            self.name, self.part_type, size, unit
        )
    }
}

#[derive(Clone, Debug, Default)]
struct SourceDiskPartitionInfo {
    name: String,
    num_sectors: usize,
    sector_size: usize,
    start_sector: usize,
}

impl SourceDiskPartitionInfo {
    fn size(&self) -> usize {
        self.num_sectors * self.sector_size
    }
}

impl Display for SourceDiskPartitionInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let (size, unit) = crate::readable_size(self.size());

        write!(
            f,
            "name: {:>15} | start: {:>10} | size: {:>5.1} {}",
            self.name, self.start_sector, size, unit
        )
    }
}
