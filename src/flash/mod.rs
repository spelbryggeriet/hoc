mod parse;

use std::ffi::OsString;
use std::fmt::{self, Display, Formatter};
use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::{fs::File, process::Command};

use anyhow::{anyhow, Context};
use heck::SnakeCase;
use serde::Deserialize;
use structopt::StructOpt;
use strum::IntoEnumIterator;
use tempfile::NamedTempFile;
use xz2::read::XzDecoder;
use zip::ZipArchive;

use crate::prelude::*;

#[derive(StructOpt)]
pub(super) struct CmdFlash {
    /// Use cached image instead of fetching it.
    #[structopt(short, long)]
    cached: bool,
}

impl CmdFlash {
    pub(super) async fn run(self, log: &mut Logger) -> AppResult<()> {
        let image = self.select_image(log).context("Selecting image")?;

        let mut image_file = self
            .fetch_image(log, image)
            .await
            .with_context(|| format!("Fetching image '{}'", image))?;

        let (image_size, raw_file) = self
            .decompress_image(log, &mut image_file, image.compression_type())
            .context("Decompressing image")?;

        if image == Image::Fedora {
            let source_disk_partition = self
                .select_source_disk_partition(log, raw_file.path(), image_size)
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

        self.flash_target_disk(log, target_disk_info, raw_file.path())
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
    ) -> AppResult<Box<dyn KnownFile>> {
        let (is_fetched, known_file): (bool, Box<dyn KnownFile>) = if !self.cached {
            let known_file = Box::new(NamedTempFile::new().context("Creating temporary file")?);
            (false, known_file)
        } else {
            crate::configure_home_dir(log).context("Configuring home directory")?;

            let cached_image_path = CACHE_DIR.join(image.description().to_snake_case());
            if !cached_image_path.exists() {
                let known_file = Box::new(
                    NamedFile::new(cached_image_path).context("Creating cached image file")?,
                );
                (false, known_file)
            } else {
                log.info(format!(
                    "Using cached image file '{}'",
                    cached_image_path.to_string_lossy()
                ))?;
                let known_file = Box::new(
                    NamedFile::open(cached_image_path).context("Opening cached image file")?,
                );
                (true, known_file)
            }
        };

        if !is_fetched {
            log.status(format!("Fetching image '{}' from '{}'", image, image.url()))?;
            let bytes = reqwest::get(image.url()).await?.bytes().await?;

            log.status(format!(
                "Writing image '{}' to file '{}'",
                image,
                known_file.path().to_string_lossy()
            ))?;
            known_file
                .as_file()
                .write(bytes.as_ref())
                .context("Writing Raspbian image to file")?;
        }

        Ok(known_file)
    }

    fn decompress_image(
        &self,
        log: &mut Logger,
        image_file: impl KnownFile,
        compression_type: CompressionType,
    ) -> AppResult<(u64, NamedTempFile)> {
        let mut raw_file = NamedTempFile::new().context("Creating temporary file")?;

        let size = match compression_type {
            CompressionType::Xz => self.decompress_xz_image(log, image_file, &mut raw_file)?,
            CompressionType::Zip => self.decompress_zip_image(log, image_file, &mut raw_file)?,
        };

        Ok((size, raw_file))
    }

    fn decompress_xz_image(
        &self,
        log: &mut Logger,
        compressed_file: impl KnownFile,
        raw_file: &mut impl KnownFile,
    ) -> AppResult<u64> {
        let mut decompressor = XzDecoder::new(compressed_file.as_file());

        log.status("Decompressing image")?;
        let size = io::copy(&mut decompressor, raw_file.as_file_mut()).with_context(|| {
            format!(
                "Decompressing image '{}' into raw file '{}'",
                compressed_file.path().to_string_lossy(),
                raw_file.path().to_string_lossy()
            )
        })?;

        Ok(size)
    }

    fn decompress_zip_image(
        &self,
        log: &mut Logger,
        compressed_file: impl KnownFile,
        raw_file: &mut impl KnownFile,
    ) -> AppResult<u64> {
        let mut archive = ZipArchive::new(compressed_file.as_file()).context("Reading Zip file")?;

        for i in 0..archive.len() {
            let mut archive_file = archive.by_index(i).context("Reading archive file")?;

            if archive_file.is_file() && archive_file.name().ends_with(".img") {
                log.status("Decompressing image")?;
                let size =
                    io::copy(&mut archive_file, raw_file.as_file_mut()).with_context(|| {
                        format!(
                            "Decompressing image '{}' into raw file '{}'",
                            compressed_file.path().to_string_lossy(),
                            raw_file.path().to_string_lossy()
                        )
                    })?;

                return Ok(size);
            }
        }

        Err(anyhow!("Image not found within Zip archive"))
    }

    fn select_source_disk_partition(
        &self,
        log: &mut Logger,
        image_path: &Path,
        image_size: u64,
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

trait KnownFile {
    fn path(&self) -> &Path;
    fn as_file(&self) -> &File;
    fn as_file_mut(&mut self) -> &mut File;
}

impl KnownFile for Box<dyn KnownFile> {
    fn path(&self) -> &Path {
        (**self).path()
    }

    fn as_file(&self) -> &File {
        (**self).as_file()
    }

    fn as_file_mut(&mut self) -> &mut File {
        (**self).as_file_mut()
    }
}

impl KnownFile for &mut Box<dyn KnownFile> {
    fn path(&self) -> &Path {
        (**self).path()
    }

    fn as_file(&self) -> &File {
        (**self).as_file()
    }

    fn as_file_mut(&mut self) -> &mut File {
        (**self).as_file_mut()
    }
}

struct NamedFile {
    path: PathBuf,
    file: File,
}

impl NamedFile {
    pub fn new(path: impl AsRef<Path>) -> AppResult<Self> {
        Ok(Self {
            path: path.as_ref().to_path_buf(),
            file: File::create(path)?,
        })
    }

    pub fn open(path: impl AsRef<Path>) -> AppResult<Self> {
        Ok(Self {
            path: path.as_ref().to_path_buf(),
            file: File::open(path)?,
        })
    }
}

impl KnownFile for NamedFile {
    fn path(&self) -> &Path {
        self.path.as_path()
    }

    fn as_file(&self) -> &File {
        &self.file
    }

    fn as_file_mut(&mut self) -> &mut File {
        &mut self.file
    }
}

impl KnownFile for NamedTempFile {
    fn path(&self) -> &Path {
        self.path()
    }

    fn as_file(&self) -> &File {
        self.as_file()
    }

    fn as_file_mut(&mut self) -> &mut File {
        self.as_file_mut()
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TargetDiskInfo {
    #[serde(rename = "DeviceIdentifier")]
    id: String,

    #[serde(rename = "Content")]
    part_type: String,

    size: u64,

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

    size: u64,

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

    size: u64,

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

    size: u64,

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
    num_sectors: u64,
    partitions: Vec<SourceDiskPartitionInfo>,
}

#[derive(Clone, Debug, Default)]
struct SourceDiskPartitionInfo {
    name: String,
    num_sectors: u64,
    sector_size: u64,
    start_sector: u64,
}

impl SourceDiskPartitionInfo {
    fn size(&self) -> u64 {
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
