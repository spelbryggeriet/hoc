use std::fmt::{self, Display, Formatter};

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

    let output = run!("file {}", os_image_file_path.to_string_lossy()).await?;
}

#[throws(anyhow::Error)]
async fn download_node_image(file: &mut File) {
    progress_scoped!("Downloading node image");

    let image_url = ubuntu_image_url(UBUNTU_VERSION);
    info!("URL: {image_url}");

    let mut image_reader = surf::get(image_url)
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
