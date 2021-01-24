pub mod prelude {
    pub use super::{
        CiBuildStage, CiConfig, CiImage, CiImageArgument, CiImagePlatform,
        CiImagePlatformArchVariant,
    };
}

use std::fmt::{self, Display, Formatter};
use std::fs;
use std::num::NonZeroU32;
use std::path::PathBuf;

use git2::Repository;
use serde::de::{self, Deserializer, Visitor};
use serde::Deserialize;

use crate::prelude::*;

pub fn get_config(repo: &Repository) -> AppResult<CiConfig> {
    let config_path = repo.path().join("../.hoc.yaml");
    anyhow::ensure!(config_path.exists(), "No Hoc config file found");

    let config_str = fs::read_to_string(config_path).context("Reading Hoc config file")?;

    Ok(serde_yaml::from_str(&config_str)?)
}

#[derive(Deserialize, Clone, Debug)]
pub struct CiConfig {
    pub version: CiVersion,
    pub build: Option<CiBuildStage>,
}

impl CiConfig {
    pub fn get_tags(&self) -> impl Iterator<Item = &str> {
        self.build
            .iter()
            .flat_map(|build| build.images.iter())
            .flat_map(|image| image.tags.iter())
            .map(String::as_str)
    }
}

impl Default for CiConfig {
    fn default() -> Self {
        CiConfig {
            version: CiVersion {
                version: NonZeroU32::new(1).unwrap(),
                beta: true,
            },
            build: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CiVersion {
    pub version: NonZeroU32,
    pub beta: bool,
}

impl<'de> Deserialize<'de> for CiVersion {
    fn deserialize<D>(deserializer: D) -> Result<CiVersion, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct CiVersionVisitor;

        impl<'de> Visitor<'de> for CiVersionVisitor {
            type Value = CiVersion;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a valid version in the format `vX` or `vX_beta`, where `X` is an integer larger than 0")
            }

            fn visit_str<E>(self, s: &str) -> Result<CiVersion, E>
            where
                E: de::Error,
            {
                use nom::bytes::complete::tag;
                use nom::character::complete::digit1;
                use nom::combinator::{map_res, opt};

                let parse_err = |e: nom::Err<(&str, nom::error::ErrorKind)>| {
                    E::custom(format!("failed parsing version: {}", e))
                };

                let (s, _) = tag("v")(s).map_err(parse_err)?;
                let (s, version) =
                    map_res(digit1, |s| str::parse::<NonZeroU32>(s))(s).map_err(parse_err)?;
                let (_, beta) = opt(tag("_beta"))(s).map_err(parse_err)?;

                Ok(CiVersion {
                    version,
                    beta: beta.is_some(),
                })
            }
        }

        deserializer.deserialize_str(CiVersionVisitor)
    }
}

impl Display for CiVersion {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "v{}", self.version)?;
        if self.beta {
            write!(f, "_beta")?;
        }
        Ok(())
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct CiBuildStage {
    pub images: Vec<CiImage>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CiImage {
    pub path: PathBuf,

    pub architecture_arg_name: String,

    pub lockfile: Option<String>,

    #[serde(default = "Vec::new")]
    pub tags: Vec<String>,

    #[serde(default = "Vec::new")]
    pub args: Vec<CiImageArgument>,

    #[serde(default = "Vec::new")]
    pub platforms: Vec<CiImagePlatform>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CiImageArgument {
    pub name: String,
    pub value: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CiImagePlatform {
    pub os: String,

    #[serde(flatten)]
    pub arch_variant: Option<CiImagePlatformArchVariant>,
}

impl Default for CiImagePlatform {
    fn default() -> Self {
        Self {
            os: "linux".into(),
            arch_variant: Some(CiImagePlatformArchVariant {
                arch: "arm".into(),
                variant: Some("v7".into()),
            }),
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct CiImagePlatformArchVariant {
    pub arch: String,
    pub variant: Option<String>,
}

impl Display for CiImagePlatform {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.os)?;
        if let Some(arch_variant) = self.arch_variant.as_ref() {
            write!(f, "/{}", arch_variant.arch)?;
            if let Some(variant) = arch_variant.variant.as_ref() {
                write!(f, "/{}", variant)?;
            }
        }
        Ok(())
    }
}
