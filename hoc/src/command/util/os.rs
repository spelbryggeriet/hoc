use std::str::FromStr;

use derive_more::Display;
use serde::{Deserialize, Serialize};
use smart_default::SmartDefault;
use structopt::StructOpt;
use thiserror::Error;

#[derive(Debug, Error)]
#[error("unknown operating system: {unknown_os}")]
pub struct ParseOsError {
    unknown_os: String,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, StructOpt, SmartDefault, Display, Serialize, Deserialize,
)]
pub enum OperatingSystem {
    #[display(fmt = "Raspberry Pi OS ({version})")]
    RaspberryPiOs {
        #[structopt(skip)]
        version: RaspberryPiOsVersion,
    },

    #[default]
    #[display(fmt = "Ubuntu ({version})")]
    Ubuntu {
        #[structopt(skip)]
        version: UbuntuVersion,
    },
}

impl FromStr for OperatingSystem {
    type Err = ParseOsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "raspberry-pi-os" => Ok(Self::RaspberryPiOs {
                version: Default::default(),
            }),
            "ubuntu" => Ok(Self::Ubuntu {
                version: Default::default(),
            }),
            _ => Err(ParseOsError {
                unknown_os: s.to_string(),
            }),
        }
    }
}

impl OperatingSystem {
    pub fn image_url(&self) -> String {
        match self {
            Self::RaspberryPiOs { version } => format!("https://downloads.raspberrypi.org/raspios_lite_arm64/images/raspios_lite_arm64-{version}/{version}-raspios-bullseye-arm64-lite.zip"),
            Self::Ubuntu { version } => format!("https://cdimage.ubuntu.com/releases/{version}/release/ubuntu-{version}-preinstalled-server-arm64+raspi.img.xz"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, SmartDefault, Display, Serialize, Deserialize)]
#[display(fmt = "{year:04}-{month:02}-{day:02}")]
pub struct RaspberryPiOsVersion {
    #[default = 2022]
    year: u32,
    #[default = 1]
    month: u32,
    #[default = 28]
    day: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, SmartDefault, Display, Serialize, Deserialize)]
#[display(fmt = "{major}.{minor:02}.{patch}")]
pub struct UbuntuVersion {
    #[default = 20]
    major: u32,
    #[default = 4]
    minor: u32,
    #[default = 4]
    patch: u32,
}
