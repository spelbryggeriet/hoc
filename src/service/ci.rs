pub mod prelude {
    pub use super::{
        CiBuildStage, CiBuildType, CiConfig, CiImage, CiImageArgument, CiImagePlatform,
        CiImagePlatformArchVariant,
    };
}

use std::fmt::{self, Display, Formatter};
use std::fs;
use std::path::PathBuf;

use git2::Repository;
use serde::Deserialize;

use crate::prelude::*;

pub fn get_config(repo: &Repository) -> AppResult<CiConfig> {
    let config_path = repo.path().join("../.h2t-ci.yaml");
    if config_path.exists() {
        let config_str = fs::read_to_string(config_path).context("Reading h2t CI config file")?;
        Ok(serde_yaml::from_str(&config_str)?)
    } else {
        info!("No h2t CI config file found, using default");
        Ok(CiConfig::default())
    }
}

#[derive(Deserialize, Clone)]
pub struct CiConfig {
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
        CiConfig { build: None }
    }
}

#[derive(Deserialize, Clone)]
pub struct CiBuildStage {
    #[serde(rename = "type")]
    pub build_type: CiBuildType,
    pub images: Vec<CiImage>,
}

#[serde(rename_all = "snake_case")]
#[derive(Deserialize, Copy, Clone)]
pub enum CiBuildType {
    Docker,
}

#[derive(Deserialize, Clone)]
pub struct CiImage {
    pub path: PathBuf,

    #[serde(default = "Vec::new")]
    pub tags: Vec<String>,

    #[serde(default = "Vec::new")]
    pub args: Vec<CiImageArgument>,

    #[serde(default = "Vec::new")]
    pub platforms: Vec<CiImagePlatform>,
}

#[derive(Deserialize, Clone)]
pub struct CiImageArgument {
    pub name: String,
    pub value: String,
}

#[derive(Deserialize, Clone)]
pub struct CiImagePlatform {
    pub os: String,

    #[serde(flatten)]
    pub arch_variant: Option<CiImagePlatformArchVariant>,
}

#[derive(Deserialize, Clone)]
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
