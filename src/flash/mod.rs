mod parse;

use std::ffi::OsString;
use std::fmt::{self, Display, Formatter};
use std::fs::File;
use std::io::{Cursor, Read, Write, Seek, SeekFrom};
use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context};
use heck::SnakeCase;
use serde::Deserialize;
use structopt::StructOpt;
use strum::IntoEnumIterator;
use xz2::read::XzDecoder;
use zip::ZipArchive;

use crate::prelude::*;

#[derive(Clone, Copy)]
enum FlashCacheState {
    Downloaded,
    Decompressed,
}

#[derive(StructOpt)]
pub(super) struct CmdFlash {}

impl CmdFlash {
    pub(super) async fn run(self, context: &mut AppContext, log: &mut Logger) -> AppResult<()> {
        let image = self.select_image(log).context("Selecting image")?;

        let image_key = image.description().to_snake_case();

        let (current_state, temp_file) = context.start_cache_processing(&image_key)?;
        if current_state < Some(FlashCacheState::Downloaded as isize) {
            self.fetch_image(log, image, temp_file.as_file_mut())
                .await
                .with_context(|| format!("Fetching image '{}'", image))?;
        }
        context.stop_cache_processing(
            &image_key,
            current_state
                .unwrap_or(FlashCacheState::Downloaded as isize)
                .max(FlashCacheState::Downloaded as isize),
        )?;

        let (current_state, temp_file) = context.start_cache_processing(&image_key)?;
        let image_size = if current_state < Some(FlashCacheState::Decompressed as isize) {
            self.decompress_image(log, image, temp_file.as_file_mut())
                .context("Decompressing image")?
        } else {
            temp_file.as_file().metadata()?.len() as usize
        };

        context.stop_cache_processing(
            &image_key,
            current_state
                .unwrap_or(FlashCacheState::Decompressed as isize)
                .max(FlashCacheState::Decompressed as isize),
        )?;

        let image_file = context.get_named_file(image_key)?;

        if image == Image::Fedora {
            let source_disk_partition = self
                .select_source_disk_partition(log, image_file.path(), image_size)
                .context("Selecting disk partition")?;
        }

        let target_disk_info = self
            .select_target_disk(log)
            .context("Selecting target disk")?;

        let disk_info_str = target_disk_info.to_string();
        log.prompt(format!(
            "Do you want to flash target disk '{}'?",
            disk_info_str
        ))?;

        self.unmount_target_disk(log, &target_disk_info)
            .context("Unmounting target disk")?;

        self.flash_target_disk(log, target_disk_info, image_file.path())
            .with_context(|| format!("Flashing target disk '{}'", disk_info_str))?;

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

    fn select_source_disk_partition(
        &self,
        log: &mut Logger,
        image_path: &Path,
        image_size: usize,
    ) -> AppResult<SourceDiskPartitionInfo> {
        let stdout = if cfg!(target_os = "macos") {
            Command::new("fdisk")
                .arg(image_path)
                .output()
                .context("Executing fdisk")?
                .stdout
        } else if cfg!(target_os = "linux") {
            Command::new("fdisk")
                .arg("-l")
                .args(&["-o", "Start,Sectors,Type"])
                .arg(image_path)
                .output()
                .context("Executing fdisk")?
                .stdout
        } else {
            anyhow::bail!("Windows not supported");
        };

        let output = String::from_utf8(stdout).context("Converting stdout to UTF-8")?;
        let (_, mut disk_info) = parse::source_disk_info(&output)
            .map_err(|e| anyhow!(e.to_string()))
            .context("Parsing disk info")?;

        let sector_size = image_size / disk_info.num_sectors;
        disk_info
            .partitions
            .iter_mut()
            .for_each(|p| p.sector_size = sector_size);
        disk_info.partitions = disk_info
            .partitions
            .into_iter()
            .filter(|p| p.num_sectors > 0)
            .collect();

        let (largest_partition_index, _) =
            disk_info
                .partitions
                .iter()
                .enumerate()
                .fold((0, 0), |(acc_i, acc_num), (i, p)| {
                    if p.num_sectors >= acc_num {
                        (i, p.num_sectors)
                    } else {
                        (acc_i, acc_num)
                    }
                });

        let index = log.choose(
            "Choose the partition where the OS lives (most likely the largest one)",
            disk_info.partitions.iter(),
            largest_partition_index,
        )?;

        Ok(disk_info.partitions.remove(index))
    }

    fn select_target_disk(&self, log: &mut Logger) -> AppResult<TargetDiskInfo> {
        let mut disk_info = if cfg!(target_os = "macos") {
            let stdout = Command::new("diskutil")
                .args(&["list", "-plist", "external", "physical"])
                .output()
                .context("Executing diskutil")?
                .stdout;

            #[derive(Deserialize)]
            #[serde(rename_all = "PascalCase")]
            struct DiskutilOutput {
                all_disks_and_partitions: Vec<TargetDiskInfo>,
            }

            let output: DiskutilOutput =
                plist::from_bytes(&stdout).context("Parsing diskutil output")?;

            output.all_disks_and_partitions
        } else if cfg!(target_os = "linux") {
            let stdout = Command::new("lsblk")
                .arg("-bOJ")
                .output()
                .context("Executing lsblk")?
                .stdout;

            #[derive(Deserialize)]
            struct LsblkOutput {
                blockdevices: Vec<TargetDiskInfo>,
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

        if disk_info.is_empty() {
            anyhow::bail!("No external physical disk mounted");
        } else {
            let index = log.choose("Choose which disk to flash", disk_info.iter(), 0)?;
            Ok(disk_info.remove(index))
        }
    }

    fn unmount_target_disk(&self, log: &mut Logger, disk_info: &TargetDiskInfo) -> AppResult<()> {
        log.status("Unmounting disk")?;

        let output = if cfg!(target_os = "macos") {
            Command::new("diskutil")
                .arg("unmountDisk")
                .arg(format!("/dev/{}", disk_info.id))
                .output()
                .context("Executing diskutil")?
        } else if cfg!(target_os = "linux") {
            anyhow::bail!("Linux not supported");
        } else {
            anyhow::bail!("Windows not supported");
        };

        anyhow::ensure!(
            output.status.success(),
            String::from_utf8_lossy(&output.stderr).into_owned()
        );

        Ok(())
    }

    fn flash_target_disk(
        &self,
        log: &mut Logger,
        disk_info: TargetDiskInfo,
        image_path: &Path,
    ) -> AppResult<()> {
        let disk_info_str = disk_info.to_string();

        log.status(format!("Flashing disk '{}'", disk_info_str))?;
        let output = if cfg!(target_family = "unix") {
            let mut arg_if = OsString::from("if=");
            arg_if.push(image_path);

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
            image_path.to_string_lossy(),
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
struct TargetDiskInfo {
    #[serde(rename = "DeviceIdentifier")]
    id: String,

    #[serde(rename = "Content")]
    part_type: String,

    size: usize,

    #[serde(default = "Vec::new")]
    partitions: Vec<TargetDiskPartitionInfo>,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Deserialize)]
struct TargetDiskInfo {
    #[serde(rename = "name")]
    id: String,

    #[serde(rename = "type")]
    part_type: String,

    size: usize,

    #[serde(default = "Vec::new", rename = "children")]
    partitions: Vec<TargetDiskPartitionInfo>,
}

impl Display for TargetDiskInfo {
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
struct TargetDiskPartitionInfo {
    #[serde(rename = "DeviceIdentifier")]
    id: String,

    #[serde(rename = "Content")]
    part_type: Option<String>,

    size: usize,

    #[serde(rename = "VolumeName")]
    name: Option<String>,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Deserialize)]
struct TargetDiskPartitionInfo {
    #[serde(rename = "name")]
    id: String,

    #[serde(rename = "type")]
    part_type: Option<String>,

    size: usize,

    #[serde(rename = "label")]
    name: Option<String>,
}

impl Display for TargetDiskPartitionInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let (size, unit) = crate::readable_size(self.size);

        write!(
            f,
            "id: {:>7} | type: {:>30} | size: {:>5.1} {} | name: {:>10}",
            self.id,
            if let Some(part_type) = self.part_type.as_ref() {
                part_type
            } else {
                ""
            },
            size,
            unit,
            if let Some(name) = self.name.as_ref() {
                name
            } else {
                ""
            }
        )?;

        Ok(())
    }
}

#[derive(Debug, Clone)]
struct SourceDiskInfo {
    num_sectors: usize,
    partitions: Vec<SourceDiskPartitionInfo>,
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
