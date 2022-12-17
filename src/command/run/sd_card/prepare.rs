use std::{
    fmt::{self, Display, Formatter},
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    net::IpAddr,
    path::{Path, PathBuf},
};

use anyhow::Error;
use xz2::read::XzDecoder;

use crate::{
    cidr::Cidr,
    context::{self, key, kv},
    prelude::*,
    util::{self, Opt},
};

const UBUNTU_VERSION: UbuntuVersion = UbuntuVersion {
    major: 20,
    minor: 4,
    patch: 5,
};

#[throws(Error)]
pub fn run() {
    let disk = choose_sd_card()?;
    if !has_system_boot_partition(&disk) || wants_to_flash()? {
        unmount_sd_card(&disk)?;
        let os_image_path = get_os_image_path()?;
        flash_image(&disk, &os_image_path)?;
    }

    let node_name = generate_node_name()?;
    let ip_address = assign_ip_address(&node_name)?;

    let partition = mount_sd_card()?;
    let mount_dir = find_mount_dir(&disk)?;

    modify_image(&mount_dir, &node_name, ip_address)?;
    unmount_partition(&partition)?;

    report(&node_name);
}

#[throws(Error)]
fn get_os_image_path() -> PathBuf {
    let (_, os_image_path) = files!("images/os").cached(get_os_image).get_or_create()?;
    os_image_path
}

#[throws(context::Error)]
fn get_os_image(file: &mut File, path: &Path, retrying: bool) {
    download_os_image(file, retrying)?;
    validate_os_image(path)?;
    decompress_xz_file(file, path)?;
}

#[throws(Error)]
fn download_os_image(file: &mut File, prompt_url: bool) {
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

    reqwest::blocking::get(os_image_url)?.copy_to(file)?;

    // Reset file.
    file.seek(SeekFrom::Start(0))?;
}

fn ubuntu_image_url<T: Display>(version: T) -> String {
    format!("https://cdimage.ubuntu.com/releases/{version}/release/ubuntu-{version}-preinstalled-server-arm64+raspi.img.xz")
}

#[throws(Error)]
fn validate_os_image(os_image_file_path: &Path) {
    progress!("Validating file type");

    let mut output = process!("file -E {}", os_image_file_path.to_string_lossy()).run()?;
    output.stdout = output.stdout.to_lowercase();

    if !output.stdout.contains("xz compressed data") {
        error!("Unsupported file type");

        let opt = select!("Do you want to inspect the file?")
            .with_options([Opt::Yes, Opt::No])
            .get()?;

        if opt == Opt::Yes {
            process!("cat {}", os_image_file_path.to_string_lossy()).run()?;
        }

        bail!("Validation failed");
    }

    info!("File is valid XZ compressed data");
}

#[throws(Error)]
fn decompress_xz_file(os_image_file: &mut File, os_image_path: &Path) {
    let decompress_progress = progress_with_handle!("Decompressing image");

    let os_image_file_ro = File::options().read(true).open(os_image_path)?;
    let mut decompressor = XzDecoder::new(os_image_file_ro);
    let mut image_data = Vec::new();
    decompressor
        .read_to_end(&mut image_data)
        .context("Reading image in XZ file")?;

    decompress_progress.finish();

    progress!("Saving decompressed image to file");

    // Truncate any existing data.
    os_image_file.set_len(0)?;

    // Write to file.
    os_image_file.write_all(&image_data)?;
}

#[throws(Error)]
fn choose_sd_card() -> DiskInfo {
    progress!("Choosing SD card");

    loop {
        let disks = macos::get_attached_disks()?;
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
fn unmount_sd_card(disk: &DiskInfo) {
    progress!("Unmounting SD card");

    let id = &disk.id;
    process!("diskutil unmountDisk {id}").run()?;
}

#[throws(Error)]
fn flash_image(disk: &DiskInfo, os_image_path: &Path) {
    let opt = select!("Do you want to flash target disk {:?}?", disk.description())
        .with_options([Opt::Yes, Opt::No])
        .get()?;
    if opt == Opt::No {
        return;
    }

    progress!("Flashing image");

    let id = &disk.id;
    process!(sudo "dd bs=1m if={os_image_path:?} of=/dev/r{id}").run()?;
}

#[throws(Error)]
fn mount_sd_card() -> DiskPartitionInfo {
    progress!("Mounting SD card");

    let partition = loop {
        let partitions = macos::get_attached_disks()?.flat_map(|disk| {
            disk.partitions
                .into_iter()
                .filter(|part| part.name == "system-boot")
        });

        let select =
            select!("Which refers to the boot partition of the disk?").with_options(partitions);
        if select.option_count() > 0 {
            break select.get()?;
        }

        error!("No mounted disk detected");

        select!("How do you want to proceed?")
            .with_option(Opt::Retry)
            .get()?;
    };

    process!("diskutil mount {}", partition.id).run()?;

    partition
}

#[throws(Error)]
fn find_mount_dir(disk: &DiskInfo) -> PathBuf {
    progress!("Finding mount directory");

    let output = process!("df").run()?;
    let mount_line = output
        .stdout
        .lines()
        .find(|line| line.contains(&disk.id))
        .with_context(|| format!("{} not mounted", disk.id))?;
    mount_line
        .split_terminator(' ')
        .last()
        .with_context(|| format!("mount point not found for {}", disk.id))?
        .into()
}

#[throws(Error)]
fn generate_node_name() -> String {
    progress!("Generating node name");

    let num_nodes = match kv!("nodes/**").get() {
        Ok(item) => item
            .into_iter()
            .filter_key_value("initialized", true)
            .count(),
        Err(kv::Error::Key(key::Error::KeyDoesNotExist(_))) => 0,
        Err(err) => throw!(err),
    };

    let node_name = format!("node-{}", util::numeral(num_nodes as u64 + 1));
    kv!("nodes/{node_name}/initialized").put(false)?;
    info!("Node name: {node_name}");

    node_name
}

#[throws(Error)]
fn assign_ip_address(node_name: &str) -> IpAddr {
    progress!("Assigning IP address");

    let start_address: IpAddr = kv!("network/start_address").get()?.convert()?;
    let used_addresses: Vec<IpAddr> = match kv!("nodes/**").get() {
        Ok(item) => item
            .into_iter()
            .filter_key_value("initialized", true)
            .try_get_key("network/start_address")
            .and_convert()
            .collect::<Result<_, _>>()?,
        Err(kv::Error::Key(key::Error::KeyDoesNotExist(_))) => Vec::new(),
        Err(err) => throw!(err),
    };
    let prefix_len: u32 = kv!("network/prefix_len").get()?.convert()?;

    let addresses = Cidr {
        ip_addr: start_address,
        prefix_len,
    };

    let mut step = 0;
    loop {
        let next_address = addresses
            .step(step)
            .context("No more IP addresses available")?;
        if !used_addresses.contains(&next_address) {
            kv!("nodes/{node_name}/network/address").put(next_address.to_string())?;
            info!("Assigned IP Address: {next_address}");
            break next_address;
        }

        step += 1;
    }
}

#[throws(Error)]
fn modify_image(mount_dir: &Path, node_name: &str, ip_address: IpAddr) {
    let opt = select!("Do you want to modify the partition mounted at {mount_dir:?}?")
        .with_options([Opt::Yes, Opt::No])
        .get()?;

    if opt == Opt::No {
        return;
    }

    progress!("Modifying image");

    let (mut pub_key_file, _) = files!("admin/ssh/pub").get()?;
    let mut pub_key = String::new();
    pub_key_file.read_to_string(&mut pub_key)?;

    // Deserialize and serialize to check for syntax errors.
    let admin_username: String = kv!("admin/username").get()?.convert()?;
    let data_map: serde_yaml::Value = serde_yaml::from_str(&format!(
        include_str!("../../../../config/cloud-init/user_data.yaml"),
        admin_username = admin_username,
        hostname = node_name,
        ssh_pub_key = pub_key,
    ))?;
    let data = serde_yaml::to_string(&data_map)?;
    let data = "#cloud-config\n".to_owned()
        + data
            .strip_prefix("---")
            .unwrap_or(&data)
            .strip_prefix("\n")
            .unwrap_or(&data);

    debug!("User data:\n{data}");

    let user_data_path = mount_dir.join("user-data");
    fs::write(&user_data_path, &data)?;

    // Deserialize and serialize to check for syntax errors.
    let gateway: IpAddr = kv!("network/gateway").get()?.convert()?;
    let gateway_ip_version = if gateway.is_ipv4() { 4 } else { 6 };
    let network_config_map: serde_yaml::Value = serde_yaml::from_str(&format!(
        include_str!("../../../../config/cloud-init/network_config.yaml"),
        ip_address = ip_address,
        gateway = gateway,
        gateway_ip_version = gateway_ip_version,
    ))?;
    let network_config = serde_yaml::to_string(&network_config_map)?;
    let network_config = network_config
        .strip_prefix("---\n")
        .unwrap_or(&network_config);

    debug!("Network config:\n{network_config}");

    let network_config_path = mount_dir.join("network-config");
    fs::write(&network_config_path, network_config)?;
}

#[throws(Error)]
fn unmount_partition(partition: &DiskPartitionInfo) {
    progress!("Unmounting partition");

    process!("sync").run()?;
    process!("diskutil unmount {}", partition.id).run()?;
}

fn report(node_name: &str) {
    info!(
        "SD card prepared. Deploy the node using the following command:\n\nhoc node deploy \
        {node_name}",
    );
}

fn has_system_boot_partition(disk: &DiskInfo) -> bool {
    disk.partitions
        .iter()
        .any(|part| part.name == "system-boot")
}

#[throws(Error)]
fn wants_to_flash() -> bool {
    let flash_anyway = Opt::Custom("Flash anyway");
    let opt = select!("Selected SD card seems to have already been flashed with Ubuntu.")
        .with_options([Opt::Custom("Skip flashing"), flash_anyway])
        .get()?;
    opt == flash_anyway
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
    pub fn get_attached_disks() -> impl Iterator<Item = DiskInfo> {
        let output = process!("diskutil list -plist external physical").run()?;
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
