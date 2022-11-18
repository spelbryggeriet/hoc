use std::{
    fmt::{self, Display, Formatter},
    fs::File as BlockingFile,
    io::{Cursor, Read},
    net::IpAddr,
    path::Path,
};

use indexmap::IndexMap;
use tokio::{
    fs::File,
    io::{self, AsyncWriteExt},
};
use xz2::read::XzDecoder;

use crate::{
    cidr::Cidr,
    context::{
        key,
        kv::{self, Item},
    },
    prelude::*,
    util,
};

const UBUNTU_VERSION: UbuntuVersion = UbuntuVersion {
    major: 20,
    minor: 4,
    patch: 5,
};

#[throws(anyhow::Error)]
pub async fn run() {
    let (_os_image_file, _os_image_file_path) =
        context_file!("images/os").cached(get_os_image).await?;

    let node_name = generate_node_name().await?;
    assign_ip_address(&node_name).await?;
}

#[throws(anyhow::Error)]
async fn get_os_image(file: &mut File, path: &Path, retrying: bool) {
    download_os_image(file, retrying).await?;
    validate_os_image(path).await?;
    decompress_xz_file(file, path).await?;
}

#[throws(anyhow::Error)]
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

#[throws(anyhow::Error)]
async fn validate_os_image(os_image_file_path: &Path) {
    progress!("Validating file type");

    let mut output = run!("file -E {}", os_image_file_path.to_string_lossy()).await?;
    output.stdout = output.stdout.to_lowercase();

    if !output.stdout.contains("xz compressed data") {
        error!("Unsupported file type");

        let inspect_file = select!("Do you want to inspect the file?")
            .with_option("Yes", || true)
            .with_option("No", || false)
            .get()?;

        if inspect_file {
            run!("cat {}", os_image_file_path.to_string_lossy()).await?;
        }

        bail!("Validation failed");
    }
}

#[throws(anyhow::Error)]
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

#[throws(anyhow::Error)]
async fn generate_node_name() -> String {
    progress!("Generating node name");

    let num_nodes = match get!("nodes/**").await {
        Ok(item) => item
            .convert::<IndexMap<_, IndexMap<_, Item>>>()?
            .values()
            .filter(|m| m.get("initialized").and_then(Item::as_bool) == Some(true))
            .count(),
        Err(kv::Error::Key(key::Error::KeyDoesNotExist(_))) => 0,
        Err(err) => throw!(err),
    };
    let node_name = format!("node-{}", util::numeral(num_nodes as u64 + 1));
    put!(false => "nodes/{node_name}/initialized").await?;
    info!("Node name: {node_name}");
    node_name
}

#[throws(anyhow::Error)]
async fn assign_ip_address(node_name: &str) {
    progress!("Assigning IP address");

    let start_address: IpAddr = get!("network/start_address")
        .await?
        .convert::<String>()?
        .parse()?;
    let used_addresses: Vec<IpAddr> = match get!("nodes/**").await {
        Ok(item) => item
            .into_iter()
            .filter(|i| i.get("initialized").and_then(Item::as_bool) == Some(true))
            .filter_map(|i| i.take("network/start_address")?.convert::<String>().ok())
            .map(|s| s.parse().map_err(Into::into))
            .collect::<anyhow::Result<_>>()?,
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
