#[macro_use]
extern crate log;

use anyhow::Context;
use git2::{Cred, RemoteCallbacks, Repository};
use shiplift::{BuildOptions, Docker};
use std::path::{Path, PathBuf};
use std::{env, fs, io};
use structopt::StructOpt;
use tokio::prelude::{Future, Stream};

const DOCKERFILE_BUILDER: &str = include_str!("../docker/Dockerfile");

#[derive(StructOpt)]
enum App {
    Build(CmdBuild),
    Deploy(CmdDeploy),
}

#[derive(StructOpt)]
struct CmdBase {
    #[structopt(long, short)]
    service: String,
}

#[derive(StructOpt)]
struct CmdBuild {
    #[structopt(flatten)]
    base: CmdBase,
}

#[derive(StructOpt)]
struct CmdDeploy {
    #[structopt(flatten)]
    base: CmdBase,
}

impl CmdBuild {
    fn run(self) -> anyhow::Result<impl Future<Item = (), Error = ()>> {
        info!("Building service '{}'", self.base.service);

        let dir = tempfile::tempdir().context("Creating temporary directory")?;

        let repo = self.clone_repo(dir.path())?;
        let build_dir = self.prepare_build_dir(repo)?;

        self.build_docker_image(build_dir)
    }

    fn clone_repo(&self, repo_path: &Path) -> anyhow::Result<Repository> {
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
        let url = format!("git@github.com:lidin/homepi-{}.git", self.base.service);
        trace!(
            "Cloning repository '{}' into directory '{}'",
            &url,
            repo_path.to_string_lossy()
        );
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
                    let msg = format!("Removing directory '{}'", path_str);
                    trace!("{}", msg);
                    fs::remove_dir_all(path).context(msg)?;
                } else {
                    let msg = format!("Removing file '{}'", path_str);
                    trace!("{}", msg);
                    fs::remove_file(path).context(msg)?;
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
        trace!("Adding Dockerfile");
        fs::write(&dockerfile_path, DOCKERFILE_BUILDER).context(format!(
            "Writing file '{}'",
            dockerfile_path.to_string_lossy()
        ))?;

        Ok(build_dir.to_path_buf())
    }

    fn build_docker_image(
        &self,
        build_dir: PathBuf,
    ) -> anyhow::Result<impl Future<Item = (), Error = ()>> {
        let docker = Docker::new();

        // Prepare build options for Docker.
        let build_dir_str = build_dir.to_str().context(format!(
            "Converting path '{}' to valid UTF-8 string",
            build_dir.to_string_lossy()
        ))?;
        let tag = "homepi/stream-manager:dev-latest";
        let options = BuildOptions::builder(build_dir_str).tag(tag).build();

        // Start the Docker build process.
        info!("Building Docker image '{}'", tag);
        let fut = docker
            .images()
            .build(&options)
            .for_each(|object| {
                if let Some(stream) = object
                    .get("stream")
                    .and_then(|v| v.as_str())
                    .filter(|s| s.trim().len() > 0)
                {
                    info!("{}", stream.trim());
                }
                Ok(())
            })
            .map_err(|e| error!("{}", e));

        Ok(fut)
    }
}

impl CmdDeploy {
    fn run(self) -> anyhow::Result<impl Future<Item = (), Error = ()>> {
        info!("Deploying service '{}'", self.base.service);
        let build_cmd = CmdBuild { base: self.base };
        build_cmd.run()
    }
}

fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();

    match App::from_args() {
        App::Build(cmd) => tokio::run(cmd.run().context("Running build command")?),
        App::Deploy(cmd) => tokio::run(cmd.run().context("Running deploy command")?),
    }

    Ok(())
}
