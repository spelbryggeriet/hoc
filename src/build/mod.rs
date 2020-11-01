mod parse;

use std::collections::HashMap;
use std::path::PathBuf;
use std::{fs, io};

use anyhow::Context;
use bollard::{service::BuildInfo, image::BuildImageOptions, auth::DockerCredentials, Docker};
use futures::stream::StreamExt;
use git2::Repository;
use structopt::StructOpt;
use tar::Builder;

use crate::prelude::*;
use crate::service::{self, ci::prelude::*};

const DOCKERFILE_BUILDER: &str = include_str!("../../docker/Dockerfile");

#[derive(StructOpt)]
pub struct CmdBuild {
    #[structopt(long, short)]
    pub service: String,
}

impl CmdBuild {
    pub async fn run(self) -> AppResult<()> {
        status!("Building service '{}'", self.service);

        let dir = tempfile::tempdir().context("Creating temporary directory")?;

        let repo = service::clone_repo(&self.service, dir.path())?;
        let ci_config = service::ci::get_config(&repo)?;
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
            let first_full_image_name = service::full_image_name(&self.service, first_tag);
            if rest_tags.len() == 0 {
                first_full_image_name
            } else {
                format!(
                    "{}&{}",
                    first_full_image_name,
                    rest_tags
                        .iter()
                        .map(|t| service::full_image_name(&self.service, t))
                        .fold(String::new(), |mut acc, t| {
                            if !acc.is_empty() {
                                acc.push_str("&")
                            }
                            acc.push_str(&t);
                            acc
                        })
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

        let registry = format!("https://{}", service::REGISTRY_DOMAIN);
        let username = input!("Username");
        let password = input!([hidden] "Password");
        let mut credentials = HashMap::new();
        credentials.insert(service::REGISTRY_DOMAIN.to_string(), DockerCredentials {
            username: Some(username),
            password: Some(password),
            serveraddress: Some(registry),
            ..Default::default()
        });

        // Start the Docker build process.
        status!("Building Docker image");
        let mut stream = docker.build_image(build_image_options, Some(credentials), Some(tar.into()));

        let mut line = String::new();
        let mut current_escape_code = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(BuildInfo {
                    error: Some(error),
                    ..
                }) => error!("{}", error),
                Ok(chunk) => {
                    if let Some(docker_stream) = chunk.stream {
                        let mut docker_stream_chunks = docker_stream.split('\n');

                        // Always append the first chunk unconditionally.
                        if let Some(chunk) = docker_stream_chunks.next() {
                            line += chunk;
                        }

                        for docker_stream_chunk in docker_stream_chunks {
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
                            line += docker_stream_chunk;
                        }
                    }
                },
                Err(error) => error!("{}", error),
            }
        }

        if line.len() > 0 {
            info!("{}", line);
        }

        Ok(())
    }
}
