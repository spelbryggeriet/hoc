use std::{
    fmt::{self, Display, Formatter},
    fs::{self, File},
    io::{Read, Seek, Write},
    net::IpAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Error;
use xz2::read::XzDecoder;

use crate::{
    cidr::Cidr,
    context::{self, fs::ContextFile, kv},
    prelude::*,
    process,
    util::{self, DiskInfo, DiskPartitionInfo, Opt},
};

const UBUNTU_VERSION: UbuntuVersion = UbuntuVersion {
    major: 20,
    minor: 4,
    patch: 5,
};

#[throws(Error)]
pub fn run() {
    process::global_settings().local_mode();

    let disk = choose_sd_card()?;
    if !has_system_boot_partition(&disk) || wants_to_flash()? {
        unmount_sd_card(&disk)?;
        let os_image_path = get_os_image_path()?;
        flash_image(&disk, &os_image_path)?;
    }

    let node_name = generate_node_name()?;
    let ip_address = assign_ip_address(&node_name)?;
    set_not_initialized(&node_name)?;

    let partition = mount_sd_card()?;
    let mount_dir = find_mount_dir(&disk)?;

    modify_image(&mount_dir, &node_name, ip_address)?;
    unmount_partition(&partition)?;

    report(&node_name);
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

#[throws(Error)]
fn choose_sd_card() -> DiskInfo {
    progress!("Choosing SD card");

    loop {
        let disks = util::get_attached_disks()?;
        let select = select!("Which disk is your SD card?").with_options(disks);
        if select.option_count() > 0 {
            break select.get()?;
        }

        error!("No mounted disk detected");

        let opt = select!("Do you want to proceed?")
            .with_options([Opt::Yes, Opt::No])
            .get()?;

        if opt == Opt::No {
            throw!(inquire::InquireError::OperationCanceled);
        }
    }
}

#[throws(Error)]
fn unmount_sd_card(disk: &DiskInfo) {
    progress!("Unmounting SD card");

    let id = &disk.id;
    process!("diskutil unmountDisk {id}").run()?;
}

#[throws(Error)]
fn get_os_image_path() -> PathBuf {
    files!("images/os")
        .cached(get_os_image)
        .get_or_create()?
        .local_path
}

#[throws(context::Error)]
fn get_os_image(file: &mut ContextFile, retrying: bool) {
    download_os_image(file, retrying)?;
    validate_os_image(file)?;
    decompress_xz_file(file)?;
}

#[throws(Error)]
fn download_os_image(file: &mut ContextFile, prompt_url: bool) {
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
    file.rewind()?;
}

fn ubuntu_image_url<T: Display>(version: T) -> String {
    format!("https://cdimage.ubuntu.com/releases/{version}/release/ubuntu-{version}-preinstalled-server-arm64+raspi.img.xz")
}

#[throws(Error)]
fn validate_os_image(os_image_file_path: &ContextFile) {
    progress!("Validating file type");

    let mut output = process!(
        "file -E {path}",
        path = &os_image_file_path.local_path.to_string_lossy(),
    )
    .run()?;
    output.stdout = output.stdout.to_lowercase();

    if !output.stdout.contains("xz compressed data") {
        error!("Unsupported file type");

        let opt = select!("Do you want to inspect the file?")
            .with_options([Opt::Yes, Opt::No])
            .get()?;

        if opt == Opt::Yes {
            process!(
                "cat {path}",
                path = os_image_file_path.local_path.to_string_lossy()
            )
            .run()?;
        }

        bail!("Validation failed");
    }

    info!("File is valid XZ compressed data");
}

#[throws(Error)]
fn decompress_xz_file(os_image_file: &mut ContextFile) {
    let decompress_progress = progress_with_handle!("Decompressing image");

    let os_image_file_ro = File::options().read(true).open(&os_image_file.local_path)?;
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
fn flash_image(disk: &DiskInfo, os_image_path: &Path) {
    let opt = select!("Do you want to flash target disk {:?}?", disk.description())
        .with_options([Opt::Yes, Opt::No])
        .get()?;
    if opt == Opt::No {
        throw!(inquire::InquireError::OperationCanceled);
    }

    progress!("Flashing image");

    let id = &disk.id;
    process!(sudo "dd bs=1m if={os_image_path:?} of=/dev/r{id}").run()?;
}

#[throws(Error)]
fn set_not_initialized(node_name: &str) {
    kv!("nodes/{node_name}/initialized").put(false)?;
}

#[throws(Error)]
fn mount_sd_card() -> DiskPartitionInfo {
    progress!("Mounting SD card");

    let partition = loop {
        let partitions = util::get_attached_disks()?.into_iter().flat_map(|disk| {
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

    const MAX_ATTEMPTS: u32 = 3;
    let mut attempts = 0;
    loop {
        let success_codes: &[i32] = if attempts < MAX_ATTEMPTS - 1 {
            &[0, 1]
        } else {
            &[0]
        };
        let output = process!("diskutil mount {id}", id = partition.id)
            .success_codes(success_codes.iter().copied())
            .run()?;
        if output.code == 0 {
            break;
        }

        attempts += 1;

        spin_sleep::sleep(Duration::from_secs(3));
    }

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

    let mut used_indices: Vec<_> = match kv!("nodes/**").get() {
        Ok(kv::Item::Map(map)) => map
            .into_keys()
            .map(|key| util::numeral_to_int(key.trim_start_matches("node-")))
            .collect::<Option<_>>()
            .context("Could not parse pending node names")?,
        Ok(_) => bail!("Could not determine available node names due to invalid context"),
        Err(context::Error::KeyDoesNotExist(_)) => Vec::new(),
        Err(err) => throw!(err),
    };

    used_indices.sort();

    let available_index = (1..)
        .zip(&used_indices)
        .find(|(i, j)| i != *j)
        .map_or(used_indices.len() as u64 + 1, |(i, _)| i);

    let node_name = format!("node-{}", util::int_to_numeral(available_index));
    info!("Node name: {node_name}");

    node_name
}

#[throws(Error)]
fn assign_ip_address(node_name: &str) -> Cidr {
    progress!("Assigning IP address");

    let start_address: IpAddr = kv!("network/start_address").get()?.convert()?;
    let prefix_len: u32 = kv!("network/prefix_len").get()?.convert()?;

    let mut used_addresses: Vec<_> = match kv!("nodes/**").get() {
        Ok(item) => item
            .into_iter()
            .try_get("network/address")
            .and_convert::<IpAddr>()
            .collect::<Result<_, _>>()?,
        Err(context::Error::KeyDoesNotExist(_)) => Vec::new(),
        Err(err) => throw!(err),
    };

    used_addresses.sort();

    let addresses = Cidr {
        ip_addr: start_address,
        prefix_len,
    };

    let available_address = (0..)
        .zip(&used_addresses)
        .map(|(i, address)| (addresses.offset(i), address))
        .find(|(a, b)| *a != Some(**b))
        .map_or(addresses.offset(used_addresses.len() as u128), |(i, _)| i)
        .context("No more IP addresses available")?;

    kv!("nodes/{node_name}/network/address").put(available_address.to_string())?;
    info!("Assigned IP Address: {available_address}");

    Cidr {
        ip_addr: available_address,
        prefix_len,
    }
}

#[throws(Error)]
fn modify_image(mount_dir: &Path, node_name: &str, ip_address: Cidr) {
    let opt = select!("Do you want to modify the partition mounted at {mount_dir:?}?")
        .with_options([Opt::Yes, Opt::No])
        .get()?;

    if opt == Opt::No {
        return;
    }

    progress!("Modifying image");

    let mut pub_key_file = files!("admin/ssh/pub").get()?;
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
    let data = "#cloud-config\n".to_owned() + data.strip_prefix("---\n").unwrap_or(&data);

    debug!("User data:\n{data}");

    let user_data_path = mount_dir.join("user-data");
    fs::write(user_data_path, &data)?;

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
    fs::write(network_config_path, network_config)?;

    let cmdline_path = mount_dir.join("cmdline.txt");
    process!(
        "sed -i '' -E 's/ *cgroup_(memory|enable)=[^ ]*//g;\
                       s/$/ cgroup_memory=1 cgroup_enable=memory/' {cmdline_path:?}"
    )
    .run()?;
}

#[throws(Error)]
fn unmount_partition(partition: &DiskPartitionInfo) {
    progress!("Unmounting partition");

    process!("sync").run()?;
    process!("diskutil unmount {id}", id = partition.id).run()?;
}

fn report(node_name: &str) {
    info!(
        "SD card prepared. Deploy the node using the following command:\
        \n\
        \n  hoc node deploy {node_name}",
    );
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
