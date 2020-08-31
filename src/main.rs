#[macro_use]
extern crate log;

mod logger;
mod parse;

use anyhow::Context;
use git2::{Cred, RemoteCallbacks, Repository};
use logger::Logger;
use reqwest::blocking::Client;
use shiplift::{BuildOptions, Docker};
use std::fmt::{self, Display, Formatter};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::{env, fs, process::Command};
use structopt::StructOpt;
use tempfile::NamedTempFile;
use tokio::prelude::{Future, Stream};

const DOCKERFILE_BUILDER: &str = include_str!("../docker/Dockerfile");
const RPI_IMAGE_URL: &str = "https://downloads.raspberrypi.org/raspios_lite_armhf_latest";

#[derive(StructOpt)]
enum App {
    Build(CmdBuild),
    Deploy(CmdDeploy),
    Flash(CmdFlash),
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

impl CmdBuild {
    fn run(self, log: &mut Logger) -> anyhow::Result<impl Future<Item = (), Error = ()>> {
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

#[derive(StructOpt)]
struct CmdDeploy {
    #[structopt(flatten)]
    base: CmdBase,
}

impl CmdDeploy {
    fn run(self, log: &mut Logger) -> anyhow::Result<impl Future<Item = (), Error = ()>> {
        info!("Deploying service '{}'", self.base.service);
        let build_cmd = CmdBuild { base: self.base };
        build_cmd.run(log)
    }
}

#[derive(StructOpt)]
struct CmdFlash {}

impl CmdFlash {
    fn run(self, log: &mut Logger) -> anyhow::Result<()> {
        let disk_info = self.select_drive(log).context("Selecting drive")?;
        let disk_info_str = disk_info.to_string();

        let image_path = self.fetch_image(log)?;

        self.flash(log, disk_info, image_path.path())
            .with_context(|| format!("Flashing disk {}", disk_info_str))?;

        Ok(())
    }

    fn fetch_image(&self, log: &mut Logger) -> anyhow::Result<NamedTempFile> {
        let client = Client::builder()
            .timeout(None)
            .build()
            .context("Building HTTP client")?;

        log.status("Fetching latest Raspbian image")?;
        let bytes = client.get(RPI_IMAGE_URL).send()?.bytes()?;

        let mut named_temp_file = NamedTempFile::new().context("Creating temporary file")?;
        named_temp_file
            .write(bytes.as_ref())
            .context("Writing Raspbian image to file")?;
        log.info(format!(
            "Raspbian image written to temporary file '{}'",
            named_temp_file.path().to_string_lossy()
        ))?;

        Ok(named_temp_file)
    }

    fn select_drive(&self, log: &mut Logger) -> anyhow::Result<DiskInfo> {
        #[cfg(target_os = "macos")]
        let stdout = Command::new("diskutil")
            .args(&["list", "external", "physical"])
            .output()
            .context("Executing diskutil")?
            .stdout;

        let output = String::from_utf8(stdout).context("Converting stdout to UTF-8")?;
        let mut disk_info = parse::disk_info(&output).context("Parsing disk info")?;

        if disk_info.is_empty() {
            anyhow::bail!("No external physical drive mounted");
        } else {
            let choice = log.choose(
                "Choose one of the following drives to flash",
                disk_info.iter(),
            )?;
            if let Some(index) = choice {
                Ok(disk_info.remove(index))
            } else {
                anyhow::bail!("User canceled operation");
            }
        }
    }

    fn flash(
        &self,
        log: &mut Logger,
        disk_info: DiskInfo,
        image_path: &Path,
    ) -> anyhow::Result<()> {
        if !log.prompt(format!("Do you want to flash '{}'?", disk_info))? {
            anyhow::bail!("User canceled operation");
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
struct DiskInfo {
    dir: String,
    id: String,
    partitions: Vec<PartitionInfo>,
    last_partition: PartitionInfo,
}

impl Display for DiskInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.id, self.last_partition.part_type)?;

        if let Some(name) = self.last_partition.name.as_ref() {
            write!(f, "{}", name)?;
        }

        write!(
            f,
            " - {}{} {}",
            self.last_partition.size.0, self.last_partition.size.1, self.last_partition.size.2
        )
    }
}

#[derive(Debug, Clone)]
struct PartitionInfo {
    index: u32,
    part_type: String,
    name: Option<String>,
    size: Size,
    id: String,
}

type Size = (String, f32, String);

fn run(log: &mut Logger) -> anyhow::Result<()> {
    match App::from_args() {
        App::Build(cmd) => tokio::run(cmd.run(log).context("Running build command")?),
        App::Deploy(cmd) => tokio::run(cmd.run(log).context("Running deploy command")?),
        App::Flash(cmd) => cmd.run(log).context("Running flash command")?,
    }

    Ok(())
}

fn main() {
    pretty_env_logger::init();

    let mut log = Logger::new();

    match run(&mut log) {
        Err(e) => log
            .error(
                e.chain()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(": "),
            )
            .expect("Failed writing error log"),
        _ => (),
    }
}
