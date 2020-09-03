mod parse;

use std::fmt::{self, Display, Formatter};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::{fs::File, process::Command};

use anyhow::{anyhow, Context};
use heck::SnakeCase;
use structopt::StructOpt;
use strum::IntoEnumIterator;
use tempfile::NamedTempFile;
use xz2::read::XzDecoder;

use crate::{logger::Logger, CACHE_DIR};

#[derive(StructOpt)]
pub(super) struct CmdFlash {
    /// Use cached image instead of fetching it.
    #[structopt(short, long)]
    cached: bool,
}

impl CmdFlash {
    pub(super) async fn run(self, log: &mut Logger) -> anyhow::Result<()> {
        let image = self.select_image(log).context("Selecting image")?;

        let image_file = self
            .fetch_image(log, image)
            .await
            .with_context(|| format!("Fetching image '{}'", image))?;

        let raw_file;
        let image_size;
        if image == Image::Fedora {
            let r = self
                .decompress_image(log, image_file.path())
                .await
                .context("Decompressing image")?;
            image_size = r.0;
            raw_file = r.1;

            let disk_partition = self
                .select_disk_partition(log, raw_file.path(), image_size)
                .context("Selecting disk partition")?;
        }

        let disk_info = self.select_drive(log).context("Selecting drive")?;

        let disk_info_str = disk_info.to_string();
        self.flash_drive(log, disk_info, image_file.path())
            .with_context(|| format!("Flashing drive '{}'", disk_info_str))?;

        Ok(())
    }

    fn select_image(&self, log: &mut Logger) -> anyhow::Result<Image> {
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
    ) -> anyhow::Result<Box<dyn KnownFile>> {
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

    async fn decompress_image(
        &self,
        log: &mut Logger,
        image_path: &Path,
    ) -> anyhow::Result<(u64, NamedTempFile)> {
        let file = File::open(image_path)
            .with_context(|| format!("Opening image file '{}'", image_path.to_string_lossy()))?;
        let mut decompressor = XzDecoder::new(file);

        log.status("Decompressing image")?;
        let mut raw_file = NamedTempFile::new().context("Creating temporary file")?;
        let size = io::copy(&mut decompressor, &mut raw_file).with_context(|| {
            format!(
                "Decompressing image '{}' into raw file '{}'",
                image_path.to_string_lossy(),
                raw_file.path().to_string_lossy()
            )
        })?;

        Ok((size, raw_file))
    }

    fn select_disk_partition(
        &self,
        log: &mut Logger,
        image_path: &Path,
        image_size: u64,
    ) -> anyhow::Result<DiskPartitionInfo> {
        let stdout = if cfg!(target_os = "macos") {
            Command::new("fdisk")
                .arg(image_path)
                .output()
                .context("Executing fdisk")?
                .stdout
        } else if cfg!(target_os = "linux") {
            // TODO: implement parsing for this
            Command::new("fdisk")
                .arg("-l")
                .arg(image_path)
                .output()
                .context("Executing fdisk")?
                .stdout
        } else {
            anyhow::bail!("Windows not supported");
        };

        let output = String::from_utf8(stdout).context("Converting stdout to UTF-8")?;
        let (_, mut disk_info) = parse::disk_info(&output)
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
            largest_partition_index
        )?;

        Ok(disk_info.partitions.remove(index))
    }

    fn select_drive(&self, log: &mut Logger) -> anyhow::Result<DriveInfo> {
        let stdout = if cfg!(target_os = "macos") {
            Command::new("diskutil")
                .args(&["list", "external", "physical"])
                .output()
                .context("Executing diskutil")?
                .stdout
        } else if cfg!(target_os = "linux") {
            // TODO: implement parsing for this
            Command::new("lsblk")
                .args(&["-OJ"])
                .output()
                .context("Executing lsblk")?
                .stdout
        } else {
            anyhow::bail!("Windows not supported");
        };

        let output = String::from_utf8(stdout).context("Converting stdout to UTF-8")?;
        let (_, mut drive_info) = parse::drive_info(&output)
            .map_err(|e| anyhow!(e.to_string()))
            .context("Parsing drive info")?;

        if drive_info.is_empty() {
            anyhow::bail!("No external physical drive mounted");
        } else {
            let index = log.choose("Choose which drive to flash", drive_info.iter(), 0)?;
            Ok(drive_info.remove(index))
        }
    }

    fn flash_drive(
        &self,
        log: &mut Logger,
        disk_info: DriveInfo,
        image_path: &Path,
    ) -> anyhow::Result<()> {
        let disk_info_str = disk_info.to_string();

        log.prompt(format!("Do you want to flash drive '{}'?", disk_info_str))?;

        log.status(format!("Flashing drive '{}'", disk_info_str))?;
        let mut handle = if cfg!(target_family = "unix") {
            Command::new("dd")
                .arg("bs=1m")
                .arg(format!("if={}", image_path.to_string_lossy()))
                .arg(format!(
                    "of=/dev/{}",
                    if cfg!(target_os = "macos") {
                        format!("r{}", disk_info.id)
                    } else {
                        disk_info.id
                    }
                ))
                .spawn()
                .context("Spawning process for dd")?
        } else {
            anyhow::bail!("Windows not supported");
        };

        let exit_status = handle.wait().context("Executing dd")?;
        anyhow::ensure!(
            exit_status.success(),
            format!("dd exited with {}", exit_status)
        );

        log.info(format!(
            "Image '{}' flashed to drive '{}'",
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
}

impl Display for Image {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.description().fmt(f)
    }
}

trait KnownFile {
    fn path(&self) -> &Path;
    fn as_file(&self) -> &File;
    fn as_file_mut(&mut self) -> &mut File;
}

struct NamedFile {
    path: PathBuf,
    file: File,
}

impl NamedFile {
    pub fn new(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Ok(Self {
            path: path.as_ref().to_path_buf(),
            file: File::create(path)?,
        })
    }

    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
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

#[derive(Debug, Clone)]
struct DriveInfo {
    dir: String,
    id: String,
    partitions: Vec<DrivePartitionInfo>,
}

impl Display for DriveInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "dir: {:>7} | id: {:>7}\n  {}",
            self.dir,
            self.id,
            "-".repeat(26)
        )?;

        for partition in &self.partitions {
            write!(f, "\n  {}", partition)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
struct DrivePartitionInfo {
    index: u32,
    part_type: String,
    name: Option<String>,
    size: Size,
    id: String,
}

impl Display for DrivePartitionInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "index: {:>3} | type: {:>30} | id: {:>10} | size: {:>5} {}",
            self.index, self.part_type, self.id, self.size.1, self.size.2
        )?;

        if let Some(name) = self.name.as_ref() {
            write!(f, " | name: {:>10}", name)?;
        }

        Ok(())
    }
}

type Size = (String, f32, String);

#[derive(Debug, Clone)]
struct DiskInfo {
    num_sectors: u64,
    partitions: Vec<DiskPartitionInfo>,
}

#[derive(Clone, Debug, Default)]
struct DiskPartitionInfo {
    index: u64,
    name: String,
    num_sectors: u64,
    sector_size: u64,
    start_sector: u64,
}

impl DiskPartitionInfo {
    fn size(&self) -> u64 {
        self.num_sectors * self.sector_size
    }
}

impl Display for DiskPartitionInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut order_thousands = 0;
        let mut size = self.size() as f32;
        while size >= 1000.0 && order_thousands < 4 {
            size /= 1000.0;
            order_thousands += 1;
        }

        let unit = match order_thousands {
            0 => "bytes",
            1 => "KB",
            2 => "MB",
            3 => "GB",
            4 => "TB",
            _ => unreachable!(),
        };

        write!(
            f,
            "index: {:>3} | name: {:>15} | start: {:>10} | size: {:>5.1} {}",
            self.index, self.name, self.start_sector, size, unit
        )
    }
}
