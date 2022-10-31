use std::{
    borrow::Cow,
    fmt::{self, Display, Formatter},
};

use async_std::{fs::File, io};

use crate::prelude::*;

const UBUNTU_VERSION: UbuntuVersion = UbuntuVersion {
    major: 20,
    minor: 4,
    patch: 4,
};

#[throws(anyhow::Error)]
pub async fn run() {
    let (_, os_image_file_path) = context_file!("images/os")
        .cached(download_node_image)
        .await?;
    let mut os_image_file_path = os_image_file_path.to_string_lossy();

    progress_scoped!("Determining file type");

    loop {
        let mut output = run!("file -E {os_image_file_path}").await?;
        output.stdout = output.stdout.to_lowercase();
        if output.stdout.contains("zip archive") {
            break;
        } else if output.stdout.contains("xz compressed data") {
            break;
        } else {
            error!("Unsupported file type");

            loop {
                let modify_url = select!("How do you want to resolve the issue?")
                    .with_option("Inspect File", || false)
                    .with_option("Modify URL", || true)
                    .get()?;

                if modify_url {
                    let (_, new_os_image_file_path) = context_file!("images/os")
                        .cached(download_node_image_custom_url)
                        .clear_if_present()
                        .await?;
                    os_image_file_path =
                        Cow::Owned(new_os_image_file_path.to_string_lossy().into_owned());
                    break;
                }

                run!("cat {os_image_file_path}").await?;
            }
        }
    }
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
async fn download_node_image(file: &mut File) {
    progress_scoped!("Downloading node image");

    let image_url = ubuntu_image_url(UBUNTU_VERSION);
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

fn ubuntu_image_url<T: Display>(version: T) -> String {
    format!("https://cdimage.ubuntu.com/releases/{version}/release/ubuntu-{version}-preinstalled-server-arm64+raspi.img.xz")
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
