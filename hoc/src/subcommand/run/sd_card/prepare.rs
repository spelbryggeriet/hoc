use std::{
    fmt::{self, Display, Formatter},
    fs::File as BlockingFile,
    io::Read as BlockingRead,
};

use async_std::{
    fs::File,
    io::{self, prelude::*, SeekFrom},
    path::{Path, PathBuf},
};
use xz2::read::XzDecoder;

use crate::prelude::*;

const UBUNTU_VERSION: UbuntuVersion = UbuntuVersion {
    major: 20,
    minor: 4,
    patch: 5,
};

#[throws(anyhow::Error)]
pub async fn run() {
    let (mut os_image_file, mut os_image_file_path) = context_file!("images/os")
        .cached(download_node_image)
        .await?;

    validate_file(&mut os_image_file, &mut os_image_file_path).await?;
    decompress_xz_file(&mut os_image_file, &os_image_file_path).await?;
}

#[throws(anyhow::Error)]
async fn download_node_image(file: &mut File) {
    progress_scoped!("Downloading node image");

    let image_url = ubuntu_image_url(UBUNTU_VERSION);
    info!("URL: {image_url}");

    fetch_into_file(image_url, file).await?
}

fn ubuntu_image_url<T: Display>(version: T) -> String {
    format!("https://cdimage.ubuntu.com/releases/{version}/release/ubuntu-{version}-preinstalled-server-arm64+raspi.img.xz")
}

#[throws(anyhow::Error)]
async fn download_node_image_custom_url(file: &mut File) {
    progress_scoped!("Downloading node image");

    let image_url: String = prompt!("URL")
        .with_initial_input(&ubuntu_image_url(UBUNTU_VERSION))
        .get()?;
    info!("URL: {image_url}");

    fetch_into_file(image_url, file).await?
}

#[throws(anyhow::Error)]
async fn fetch_into_file(url: String, file: &mut File) {
    let mut image_reader = surf::get(url)
        .send()
        .await
        .unwrap()
        .take_body()
        .into_reader();
    io::copy(&mut image_reader, file).await?;
}

#[throws(anyhow::Error)]
async fn validate_file(os_image_file: &mut File, os_image_file_path: &mut PathBuf) {
    progress_scoped!("Validating file type");

    loop {
        let mut output = run!("file -E {}", os_image_file_path.to_string_lossy()).await?;
        output.stdout = output.stdout.to_lowercase();
        if output.stdout.contains("xz compressed data") {
            break;
        }

        error!("Unsupported file type");

        loop {
            let modify_url = select!("How do you want to resolve the issue?")
                .with_option("Inspect File", || false)
                .with_option("Modify URL", || true)
                .get()?;

            if modify_url {
                (*os_image_file, *os_image_file_path) = context_file!("images/os")
                    .cached(download_node_image_custom_url)
                    .clear_if_present()
                    .await?;
                break;
            }

            run!("cat {}", os_image_file_path.to_string_lossy()).await?;
        }
    }
}

#[throws(anyhow::Error)]
async fn decompress_xz_file(image_file: &mut File, image_path: &Path) {
    let read_progress = progress!("Reading XZ file");

    let blocking_image_file = BlockingFile::open(image_path)?;
    let mut decompressor = XzDecoder::new(blocking_image_file);

    let decompress_progress = progress!("Decompressing image");

    let mut image_data = Vec::new();
    decompressor
        .read_to_end(&mut image_data)
        .context("Reading image in XZ file")?;

    decompress_progress.finish();
    read_progress.finish();

    progress_scoped!("Saving decompressed image to file");

    image_file.seek(SeekFrom::Start(0)).await?;
    image_file.set_len(0).await?;
    image_file.write_all(&image_data).await?;
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
