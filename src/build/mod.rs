mod parse;

use std::path::{Path, PathBuf};
use std::{env, fs, io};

use anyhow::Context;
use futures::stream::StreamExt;
use git2::{Cred, RemoteCallbacks, Repository};
use shiplift::{BuildOptions, Docker};
use structopt::StructOpt;

use crate::logger::Logger;

const DOCKERFILE_BUILDER: &str = include_str!("../../docker/Dockerfile");

#[derive(StructOpt)]
pub(super) struct CmdBuild {
    #[structopt(long, short)]
    pub(super) service: String,
}

impl CmdBuild {
    pub(super) async fn run(self, log: &mut Logger) -> anyhow::Result<()> {
        log.status(format!("Building service '{}'", self.service))?;

        let dir = tempfile::tempdir().context("Creating temporary directory")?;

        let repo = self.clone_repo(log, dir.path())?;
        let build_dir = self.prepare_build_dir(repo)?;

        self.build_docker_image(log, build_dir).await
    }

    fn clone_repo(&self, log: &mut Logger, repo_path: &Path) -> anyhow::Result<Repository> {
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
        let url = format!("git@github.com:lidin/homepi-{}.git", self.service);
        log.status(format!(
            "Cloning repository '{}' into directory '{}'",
            &url,
            repo_path.to_string_lossy()
        ))?;
        builder
            .clone(&url, repo_path)
            .context(format!("Cloning repository '{}'", &url))
    }

    fn prepare_build_dir(&self, repo: Repository) -> anyhow::Result<PathBuf> {
        // Remove any ignored files.
        let mut paths = vec![repo.path().to_path_buf()];
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

        let build_dir = repo.path().parent().context(format!(
            "Retrieving parent directory to git repository '{}'",
            repo.path().to_string_lossy()
        ))?;

        // Add Dockerfile.
        let dockerfile_path = build_dir.join("Dockerfile");
        fs::write(&dockerfile_path, DOCKERFILE_BUILDER).context(format!(
            "Writing file '{}'",
            dockerfile_path.to_string_lossy()
        ))?;

        Ok(build_dir.to_path_buf())
    }

    async fn build_docker_image(&self, log: &mut Logger, build_dir: PathBuf) -> anyhow::Result<()> {
        let docker = Docker::new();

        // Prepare build options for Docker.
        let build_dir_str = build_dir.to_str().context(format!(
            "Converting path '{}' to valid UTF-8 string",
            build_dir.to_string_lossy()
        ))?;
        let tag = "homepi/stream-manager:dev-latest";
        let options = BuildOptions::builder(build_dir_str).tag(tag).build();

        // Start the Docker build process.
        log.status(format!("Building Docker image '{}'", tag))?;
        let images = docker.images();
        let mut stream = images.build(&options);

        let mut line = String::new();
        let mut current_escape_code = None;
        while let Some(object) = stream.next().await {
            match object {
                Ok(v) => {
                    if let Some(stream) = v.get("stream").and_then(|v| v.as_str()) {
                        let mut chunks = stream.split('\n');

                        // Always append the first chunk unconditionally.
                        if let Some(chunk) = chunks.next() {
                            line += chunk;
                        }

                        for chunk in chunks {
                            info!(
                                "{}{}",
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
