use std::collections::HashMap;
use std::path::PathBuf;
use std::{fs, io};

use anyhow::Context;
use bollard::{auth::DockerCredentials, image::BuildImageOptions, service::BuildInfo, Docker};
use futures::stream::StreamExt;
use git2::Repository;
use structopt::StructOpt;
use tar::Builder;

use crate::prelude::*;
use crate::service::{self, ci::prelude::*};

const DOCKERFILE_BUILDER: &str = include_str!("../docker/Dockerfile");

#[derive(StructOpt)]
pub struct CmdBuild {
    #[structopt(long, short)]
    pub service: String,

    #[structopt(long, short, default_value = "master")]
    pub branch: String,
}

impl CmdBuild {
    pub async fn run(self) -> AppResult<()> {
        status!("Building service");
        labelled_info!("Name", self.service);

        let dir = tempfile::tempdir().context("Creating temporary directory")?;

        let repo = service::clone_repo(&self.service, &self.branch, dir.path())?;
        let ci_config = service::ci::get_config(&repo)?;
        let build_dir = self.prepare_build_dir(&repo, &ci_config)?;

        match ci_config.build {
            Some(CiBuildStage { mut images }) => {
                let image = images.remove(0);

                let mut args = image.args;
                args.push(CiImageArgument {
                    name: image.architecture_arg_name,
                    value: image
                        .platforms
                        .get(0)
                        .map(|p| p.to_string())
                        .unwrap_or_default(),
                });

                self.build_docker_image(build_dir.join(&image.path), image.tags.clone(), args)
                    .await
            }
            None => {
                self.build_docker_image(build_dir, Vec::default(), Vec::default())
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

        if let Some(build) = &ci_config.build {
            for image in build.images.iter() {
                if let Some(lockfile) = &image.lockfile {
                    let msg = format!("Copying Cargo.lock file from '{}'", lockfile);
                    info!(msg);

                    let image_dir = build_dir.join(&image.path);
                    let lockfile_dir = image_dir.join(lockfile);

                    fs::copy(&lockfile_dir, image_dir).context(msg)?;
                }
            }
        } else {
            // Add Dockerfile.
            info!("Adding default Dockerfile");
            let dockerfile_path = build_dir.join("Dockerfile");
            fs::write(&dockerfile_path, DOCKERFILE_BUILDER)
                .context(format!("Writing file '{}'", dockerfile_path.display()))?;
        }

        Ok(build_dir.to_path_buf())
    }

    async fn build_docker_image(
        &self,
        build_dir: PathBuf,
        tags: Vec<String>,
        args: Vec<CiImageArgument>,
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
            buildargs: build_image_args, // TODO: fix correct platform //platform.unwrap_or_default().to_string(),
            ..Default::default()
        };

        // Prepare tarball.
        status!("Packing build directory into tarball");
        let mut tar_builder = Builder::new(Vec::new());
        tar_builder.append_dir_all(".", build_dir)?;
        let tar = tar_builder.into_inner()?;

        let registry = format!("https://{}", service::REGISTRY_DOMAIN);
        let username = input!("Username");
        let password = hidden_input!("Password");
        let mut credentials = HashMap::new();
        credentials.insert(
            service::REGISTRY_DOMAIN.to_string(),
            DockerCredentials {
                username: Some(username),
                password: Some(password),
                serveraddress: Some(registry),
                ..Default::default()
            },
        );

        // Start the Docker build process.
        status!("Building Docker image");
        let mut stream =
            docker.build_image(build_image_options, Some(credentials), Some(tar.into()));

        let log_stream = LOG.stream();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(BuildInfo {
                    error: Some(error), ..
                }) => {
                    error!("{}", error)
                }
                Ok(chunk) => {
                    if let Some(s) = chunk.stream {
                        log_stream.process(s)
                    }
                }
                Err(error) => {
                    error!("{}", error)
                }
            }
        }

        Ok(())
    }
}
