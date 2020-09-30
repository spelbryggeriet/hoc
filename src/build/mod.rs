mod parse;

use std::collections::HashMap;
use std::fmt::{self, Display, Formatter};
use std::path::{Path, PathBuf};
use std::{env, fs, io};

use anyhow::Context;
use bollard::{image::BuildImageOptions, service::ProgressDetail, Docker};
use futures::stream::StreamExt;
use git2::{Cred, RemoteCallbacks, Repository};
use serde::Deserialize;
use structopt::StructOpt;
use tar::Builder;

use crate::prelude::*;

const DOCKERFILE_BUILDER: &str = include_str!("../../docker/Dockerfile");

fn format_tag(service: &str, tag: &str) -> String {
    format!("registry.gitlab.com/lidin-homepi/{}:{}", service, tag)
}

#[derive(Deserialize, Clone)]
struct CiConfig {
    build: Option<CiBuildStage>,
}

impl Default for CiConfig {
    fn default() -> Self {
        CiConfig { build: None }
    }
}

#[derive(Deserialize, Clone)]
struct CiBuildStage {
    #[serde(rename = "type")]
    build_type: CiBuildType,
    images: Vec<CiImage>,
}

#[serde(rename_all = "snake_case")]
#[derive(Deserialize, Copy, Clone)]
enum CiBuildType {
    Docker,
}

#[derive(Deserialize, Clone)]
struct CiImage {
    path: PathBuf,

    #[serde(default = "Vec::new")]
    tags: Vec<String>,

    #[serde(default = "Vec::new")]
    args: Vec<CiImageArgument>,

    #[serde(default = "Vec::new")]
    platforms: Vec<CiImagePlatform>,
}

#[derive(Deserialize, Clone)]
struct CiImageArgument {
    name: String,
    value: String,
}

#[derive(Deserialize, Clone)]
struct CiImagePlatform {
    os: String,

    #[serde(flatten)]
    arch_variant: Option<CiImagePlatformArchVariant>,
}

#[derive(Deserialize, Clone)]
struct CiImagePlatformArchVariant {
    arch: String,
    variant: Option<String>,
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

#[derive(StructOpt)]
pub(super) struct CmdBuild {
    #[structopt(long, short)]
    pub(super) service: String,
}

impl CmdBuild {
    pub(super) async fn run(self) -> AppResult<()> {
        status!("Building service '{}'", self.service);

        let dir = tempfile::tempdir().context("Creating temporary directory")?;

        let repo = self.clone_repo(dir.path())?;
        let ci_config = self.get_ci_config(&repo)?;
        let build_dir = self.prepare_build_dir(&repo, &ci_config)?;

        match ci_config.build {
            Some(CiBuildStage {
                build_type: CiBuildType::Docker,
                mut images,
            }) => {
                let image = images.remove(0);
                self.build_docker_image(
                    build_dir.join(&image.path),
                    image.tags.clone(),
                    image.args.clone(),
                    image.platforms.get(0).cloned(),
                )
                .await
            }
            None => {
                self.build_docker_image(build_dir, Vec::default(), Vec::default(), None)
                    .await
            }
        }
    }

    fn clone_repo(&self, repo_path: &Path) -> AppResult<Repository> {
        // Prepare callbacks.
        let mut callbacks = RemoteCallbacks::new();
        callbacks.credentials(|_url, username_from_url, _allowed_types| {
            Cred::ssh_key(
                username_from_url.unwrap(),
                None,
                std::path::Path::new(&format!("{}/.ssh/id_rsa", env::var("HOME").unwrap())),
                None,
            )
        });

        // Prepare fetch options.
        let mut fo = git2::FetchOptions::new();
        fo.remote_callbacks(callbacks);

        // Prepare builder.
        let mut builder = git2::build::RepoBuilder::new();
        builder.fetch_options(fo);

        // Clone the project.
        let url = format!("git@gitlab.com:lidin-homepi/{}.git", self.service);
        status!(
            "Cloning repository '{}' into directory '{}'",
            &url,
            repo_path.to_string_lossy()
        );
        builder
            .clone(&url, repo_path)
            .context(format!("Cloning repository '{}'", &url))
    }

    fn get_ci_config(&self, repo: &Repository) -> AppResult<CiConfig> {
        let config_path = repo.path().join("../.h2t-ci.yaml");
        if config_path.exists() {
            let config_str =
                fs::read_to_string(config_path).context("Reading h2t CI config file")?;
            Ok(serde_yaml::from_str(&config_str)?)
        } else {
            info!("No h2t CI config file found, using default");
            Ok(CiConfig::default())
        }
    }

    fn prepare_build_dir(&self, repo: &Repository, ci_config: &CiConfig) -> AppResult<PathBuf> {
        let build_dir = repo
            .path()
            .parent()
            .context("Parent directory does not exist")?;

        // Remove any ignored files.
        let mut paths = vec![build_dir.to_path_buf()];
        while let Some(path) = paths.pop() {
            let path_str = path.to_string_lossy();
            if repo
                .is_path_ignored(&path)
                .context(format!("Checking if path '{}' is ignored by git", path_str))?
            {
                if path.is_dir() {
                    fs::remove_dir_all(&path)
                        .with_context(|| format!("Removing directory '{}'", path_str))?;
                } else {
                    fs::remove_file(&path)
                        .with_context(|| format!("Removing file '{}'", path_str))?;
                }
            } else if path.is_dir() {
                let entries: Vec<_> = fs::read_dir(&path)
                    .context(format!("Reading directory '{}'", path_str))?
                    .collect::<io::Result<_>>()
                    .context("Reading directory entry")?;
                paths.extend(entries.iter().map(|e| e.path()));
            }
        }

        if ci_config.build.is_none() {
            // Add Dockerfile.
            let dockerfile_path = build_dir.join("Dockerfile");
            fs::write(&dockerfile_path, DOCKERFILE_BUILDER).context(format!(
                "Writing file '{}'",
                dockerfile_path.to_string_lossy()
            ))?;
        }

        Ok(build_dir.to_path_buf())
    }

    async fn build_docker_image(
        &self,
        build_dir: PathBuf,
        tags: Vec<String>,
        args: Vec<CiImageArgument>,
        platform: Option<CiImagePlatform>,
    ) -> AppResult<()> {
        let docker = Docker::connect_with_unix_defaults()?;

        // Prepare build options for Docker.
        let build_image_args: HashMap<_, _> =
            args.into_iter().map(|arg| (arg.name, arg.value)).collect();

        let t = if let Some((first_tag, rest_tags)) = tags.split_first() {
            if rest_tags.len() == 0 {
                format_tag(&self.service, first_tag)
            } else {
                format!(
                    "{}&{}",
                    format_tag(&self.service, first_tag),
                    rest_tags.iter().map(|t| format_tag(&self.service, t)).fold(
                        String::new(),
                        |mut acc, t| {
                            if !acc.is_empty() {
                                acc.push_str("&")
                            }
                            acc.push_str(&t);
                            acc
                        }
                    )
                )
            }
        } else {
            String::new()
        };

        let build_image_options = BuildImageOptions {
            t,
            pull: true,
            buildargs: build_image_args,
            platform: platform.map(|p| p.to_string()).unwrap_or_default(),
            ..Default::default()
        };

        // Prepare tarball.
        status!("Packing build directory into tarball");
        let mut tar_builder = Builder::new(Vec::new());
        tar_builder.append_dir_all(".", build_dir)?;
        let tar = tar_builder.into_inner()?;

        // Start the Docker build process.
        status!("Building Docker image");
        let mut build = docker.build_image(build_image_options, None, Some(tar.into()));

        let mut line = String::new();
        let mut current_escape_code = None;
        while let Some(object) = build.next().await {
            match object {
                Ok(v) => {
                    if let Some(stream) = v.stream {
                        let mut chunks = stream.split('\n');

                        // Always append the first chunk unconditionally.
                        if let Some(chunk) = chunks.next() {
                            line += chunk;
                        }

                        for chunk in chunks {
                            info!(
                                "{}{}\u{1b}[0m",
                                current_escape_code.as_ref().unwrap_or(&String::new()),
                                line
                            );
                            if let Some(escape_code) = parse::last_ansi_escape_code(&line) {
                                current_escape_code =
                                    Some(escape_code).filter(|t| t != "\u{1b}[0m");
                            }

                            line.clear();
                            line += chunk;
                        }
                    }
                }
                Err(e) => error!("{}", e),
            }
        }

        if line.len() > 0 {
            info!("{}", line);
        }

        Ok(())
    }
}
