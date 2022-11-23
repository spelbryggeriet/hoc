use std::{
    fmt::{self, Display, Formatter},
    fs::File as BlockingFile,
    io::{Cursor, Read},
    net::IpAddr,
    path::Path,
};

use anyhow::Error;
use tokio::{
    fs::File,
    io::{self, AsyncWriteExt},
};
use xz2::read::XzDecoder;

use crate::{
    cidr::Cidr,
    context::{key, kv},
    prelude::*,
    util::{self, Opt},
};

const UBUNTU_VERSION: UbuntuVersion = UbuntuVersion {
    major: 20,
    minor: 4,
    patch: 5,
};

#[throws(Error)]
pub async fn run() {
    let (_os_image_file, os_image_path) = context_file!("images/os").cached(get_os_image).await?;

    let node_name = generate_node_name().await?;
    assign_ip_address(&node_name).await?;
    flash_image(&os_image_path).await?;
}

#[throws(Error)]
async fn get_os_image(file: &mut File, path: &Path, retrying: bool) {
    download_os_image(file, retrying).await?;
    validate_os_image(path).await?;
    decompress_xz_file(file, path).await?;
}

#[throws(Error)]
async fn download_os_image(file: &mut File, prompt_url: bool) {
    progress!("Downloading OS image");

    let os_image_url = if prompt_url {
        prompt!("URL")
            .with_initial_input(&ubuntu_image_url(UBUNTU_VERSION))
            .get()?
    } else {
        let url = ubuntu_image_url(UBUNTU_VERSION);
        info!("URL: {url}");
        url
    };

    let mut os_image_reader = Cursor::new(reqwest::get(os_image_url).await?.bytes().await?);
    io::copy(&mut os_image_reader, file).await?;
}

fn ubuntu_image_url<T: Display>(version: T) -> String {
    format!("https://cdimage.ubuntu.com/releases/{version}/release/ubuntu-{version}-preinstalled-server-arm64+raspi.img.xz")
}

#[throws(Error)]
async fn validate_os_image(os_image_file_path: &Path) {
    progress!("Validating file type");

    let mut output = cmd!("file -E {}", os_image_file_path.to_string_lossy()).await?;
    output.stdout = output.stdout.to_lowercase();

    if !output.stdout.contains("xz compressed data") {
        error!("Unsupported file type");

        let opt = select!("Do you want to inspect the file?")
            .with_options([Opt::Yes, Opt::No])
            .get()?;

        if opt == Opt::Yes {
            cmd!("cat {}", os_image_file_path.to_string_lossy()).await?;
        }

        bail!("Validation failed");
    }
}

#[throws(Error)]
async fn decompress_xz_file(os_image_file: &mut File, os_image_path: &Path) {
    let decompress_progress = progress_with_handle!("Decompressing image");

    let os_image_file_ro = BlockingFile::options().read(true).open(os_image_path)?;
    let mut decompressor = XzDecoder::new(os_image_file_ro);
    let mut image_data = Vec::new();
    decompressor
        .read_to_end(&mut image_data)
        .context("Reading image in XZ file")?;

    decompress_progress.finish();

    progress!("Saving decompressed image to file");

    os_image_file.set_len(0).await?;
    os_image_file.write_all(&image_data).await?;
}

#[throws(Error)]
async fn generate_node_name() -> String {
    progress!("Generating node name");

    let num_nodes = match get!("nodes/**").await {
        Ok(item) => item
            .into_iter()
            .filter_key_value("initialized", true)
            .count(),
        Err(kv::Error::Key(key::Error::KeyDoesNotExist(_))) => 0,
        Err(err) => throw!(err),
    };

    let node_name = format!("node-{}", util::numeral(num_nodes as u64 + 1));
    put!(false => "nodes/{node_name}/initialized").await?;
    info!("Node name: {node_name}");

    node_name
}

#[throws(Error)]
async fn assign_ip_address(node_name: &str) {
    progress!("Assigning IP address");

    let start_address: IpAddr = get!("network/start_address").await?.convert()?;
    let used_addresses: Vec<IpAddr> = match get!("nodes/**").await {
        Ok(item) => item
            .into_iter()
            .filter_key_value("initialized", true)
            .try_get_key("network/start_address")
            .and_convert()
            .collect::<Result<_, _>>()?,
        Err(kv::Error::Key(key::Error::KeyDoesNotExist(_))) => Vec::new(),
        Err(err) => throw!(err),
    };
    let prefix_len: u32 = get!("network/prefix_len").await?.convert()?;

    let addresses = Cidr {
        ip_addr: start_address,
        prefix_len,
    };
    for step in 0.. {
        let next_address = addresses
            .step(step)
            .context("No more IP addresses available")?;
        if !used_addresses.contains(&next_address) {
            put!(next_address.to_string() => "nodes/{node_name}/network/address").await?;
            info!("Assigned IP Address: {next_address}");
            break;
        }
    }
}

#[throws(Error)]
async fn flash_image(os_image_path: &Path) {
    progress!("Flashing image");

    let disk = choose_sd_card().await?;
    unmount_sd_card(&disk).await?;
    flash_sd_card(&disk, os_image_path).await?;
}

#[throws(Error)]
async fn choose_sd_card() -> DiskInfo {
    progress!("Choosing SD card");

    loop {
        let disks = macos::get_attached_disks().await?;
        let select = select!("Which disk is your SD card?").with_options(disks);
        if select.option_count() > 0 {
            break select.get()?;
        }

        error!("No mounted disk detected");

        select!("How do you want to proceed?")
            .with_option(Opt::Retry)
            .get()?;
    }
}

#[throws(Error)]
async fn unmount_sd_card(disk: &DiskInfo) {
    progress!("Unmounting SD card");

    let id = &disk.id;
    cmd!("diskutil unmountDisk {id}").await?;
}

#[throws(Error)]
async fn flash_sd_card(disk: &DiskInfo, os_image_path: &Path) {
    let opt = select!("Do you want to flash target disk {:?}?", disk.description())
        .with_options([Opt::Yes, Opt::No])
        .get()?;
    if opt == Opt::No {
        return;
    }

    let id = &disk.id;
    cmd!(sudo "dd bs=1m if={os_image_path:?} of=/dev/r{id}").await?;
}

fn unnamed_if_empty<S: AsRef<str> + ?Sized>(name: &S) -> String {
    if name.as_ref().trim().is_empty() {
        "<unnamed>".to_owned()
    } else {
        format!(r#""{}""#, name.as_ref())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UbuntuVersion {
    major: u32,
    minor: u32,
    patch: u32,
}

impl Display for UbuntuVersion {
    #[throws(fmt::Error)]
    fn fmt(&self, f: &mut Formatter) {
        write!(
            f,
            "{major}.{minor:02}.{patch}",
            major = self.major,
            minor = self.minor,
            patch = self.patch,
        )?;
    }
}

#[derive(Debug, Clone)]
pub struct DiskInfo {
    pub id: String,
    pub part_type: String,
    pub name: String,
    pub size: usize,
    pub partitions: Vec<DiskPartitionInfo>,
}

#[derive(Debug, Clone)]
pub struct DiskPartitionInfo {
    pub id: String,
    pub size: usize,
    pub name: String,
}

impl DiskInfo {
    pub fn description(&self) -> String {
        let mut desc = format!("{}: ", self.id);
        desc += &unnamed_if_empty(&self.name);
        if !self.partitions.is_empty() {
            desc += &format!(
                " ({} partition{}: {})",
                self.partitions.len(),
                if self.partitions.len() == 1 { "" } else { "s" },
                self.partitions
                    .iter()
                    .map(|p| unnamed_if_empty(&p.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        desc + &format!(", {:.2} GB", self.size as f64 / 1e9)
    }
}

impl Display for DiskInfo {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        self.description().fmt(f)
    }
}

impl DiskPartitionInfo {
    fn description(&self) -> String {
        format!(
            "{}: {} ({:.2} GB)",
            self.id,
            unnamed_if_empty(&self.name),
            self.size as f64 / 1e9,
        )
    }
}

impl Display for DiskPartitionInfo {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        self.description().fmt(f)
    }
}

mod macos {
    use serde::Deserialize;

    use super::*;

    #[throws(Error)]
    pub async fn get_attached_disks() -> impl Iterator<Item = DiskInfo> {
        let output = cmd!("diskutil list -plist external physical")
            .hide_output()
            .await?;
        let diskutil_output: DiskutilOutput = plist::from_bytes(output.stdout.as_bytes())?;
        diskutil_output
            .all_disks_and_partitions
            .into_iter()
            .map(Into::into)
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct DiskutilOutput {
        all_disks_and_partitions: Vec<DiskutilDisk>,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct DiskutilDisk {
        pub device_identifier: String,
        #[serde(default = "String::new")]
        pub volume_name: String,
        pub size: usize,
        pub content: String,
        #[serde(default = "Vec::new")]
        pub partitions: Vec<DiskutilPartition>,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct DiskutilPartition {
        pub device_identifier: String,
        #[serde(default = "String::new")]
        pub volume_name: String,
        pub size: usize,
    }

    impl From<DiskutilDisk> for DiskInfo {
        fn from(disk: DiskutilDisk) -> Self {
            Self {
                id: disk.device_identifier,
                name: disk.volume_name,
                size: disk.size,
                part_type: disk.content,
                partitions: disk.partitions.into_iter().map(Into::into).collect(),
            }
        }
    }

    impl From<DiskutilPartition> for DiskPartitionInfo {
        fn from(partition: DiskutilPartition) -> Self {
            Self {
                id: partition.device_identifier,
                name: partition.volume_name,
                size: partition.size,
            }
        }
    }
}
