#[macro_use]
extern crate log;

mod logger;
mod parse;

use anyhow::Context;
use futures::stream::StreamExt;
use git2::{Cred, RemoteCallbacks, Repository};
use logger::Logger;
use shiplift::{BuildOptions, Docker};
use std::fmt::{self, Display, Formatter};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::{env, fs, process::Command};
use structopt::StructOpt;
use tempfile::NamedTempFile;

const DOCKERFILE_BUILDER: &str = include_str!("../docker/Dockerfile");
const FEDORA_IMAGE_URL: (&str, &str) = ("Fedora 32", "https://download.fedoraproject.org/pub/fedora/linux/releases/32/Server/armhfp/images/Fedora-Server-armhfp-32-1.6-sda.raw.xz");
const RASPBIAN_IMAGE_URL: (&str, &str) = (
    "Raspbian Latest",
    "https://downloads.raspberrypi.org/raspios_lite_armhf_latest",
);

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
    async fn run(self, log: &mut Logger) -> anyhow::Result<()> {
        log.status(format!("Building service '{}'", self.base.service))?;

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
        let url = format!("git@github.com:lidin/homepi-{}.git", self.base.service);
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
                            info!("{}", line);

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

#[derive(StructOpt)]
struct CmdDeploy {
    #[structopt(flatten)]
    base: CmdBase,
}

impl CmdDeploy {
    async fn run(self, log: &mut Logger) -> anyhow::Result<()> {
        log.status(format!("Deploying service '{}'", self.base.service))?;
        let build_cmd = CmdBuild { base: self.base };
        build_cmd.run(log).await
    }
}

#[derive(StructOpt)]
struct CmdFlash {}

impl CmdFlash {
    async fn run(self, log: &mut Logger) -> anyhow::Result<()> {
        let disk_info = self.select_drive(log).context("Selecting drive")?;
        let disk_info_str = disk_info.to_string();

        let url = self.select_image(log).context("Selecting image")?;
        let image_path = self
            .fetch_image(log, url)
            .await
            .with_context(|| format!("Fetching image '{}'", url.0))?;

        self.flash(log, disk_info, image_path.path())
            .with_context(|| format!("Flashing drive '{}'", disk_info_str))?;

        Ok(())
    }

    fn select_image(&self, log: &mut Logger) -> anyhow::Result<(&'static str, &'static str)> {
        let index = log.choose(
            "Choose which operating image to download",
            &[FEDORA_IMAGE_URL.0, RASPBIAN_IMAGE_URL.0],
        )?;

        Ok([FEDORA_IMAGE_URL, RASPBIAN_IMAGE_URL][index])
    }

    async fn fetch_image<'a: 'b, 'b>(
        &'a self,
        log: &'b mut Logger,
        url: (&'static str, &'static str),
    ) -> anyhow::Result<NamedTempFile> {
        log.status(format!("Fetching image '{}'", url.0))?;
        let bytes = reqwest::get(url.1).await?.bytes().await?;

        let mut named_temp_file = NamedTempFile::new().context("Creating temporary file")?;
        named_temp_file
            .write(bytes.as_ref())
            .context("Writing Raspbian image to file")?;
        log.info(format!(
            "Image '{}' written to temporary file '{}'",
            url.0,
            named_temp_file.path().to_string_lossy()
        ))?;

        Ok(named_temp_file)
    }

    fn select_drive(&self, log: &mut Logger) -> anyhow::Result<DiskInfo> {
        let stdout = if cfg!(target_os = "macos") {
            Command::new("diskutil")
                .args(&["list", "external", "physical"])
                .output()
                .context("Executing diskutil")?
                .stdout
        } else if cfg!(target_os = "linux") {
            // TODO: implement parsing for this
            Command::new("lsblk")
                .args(&["-OJ"])
                .output()
                .context("Executing lsblk")?
                .stdout
        } else {
            anyhow::bail!("Windows not supported");
        };

        let output = String::from_utf8(stdout).context("Converting stdout to UTF-8")?;
        let mut disk_info = parse::disk_info(&output).context("Parsing disk info")?;

        if disk_info.is_empty() {
            anyhow::bail!("No external physical drive mounted");
        } else {
            let index = log.choose(
                "Choose one of the following drives to flash",
                disk_info.iter(),
            )?;
            Ok(disk_info.remove(index))
        }
    }

    fn flash(
        &self,
        log: &mut Logger,
        disk_info: DiskInfo,
        image_path: &Path,
    ) -> anyhow::Result<()> {
        let disk_info_str = disk_info.to_string();

        log.prompt(format!("Do you want to flash drive '{}'?", disk_info_str))?;

        log.status(format!("Flashing drive '{}'", disk_info_str))?;
        let mut handle = if cfg!(target_family = "unix") {
            Command::new("dd")
                .arg("bs=1m")
                .arg(format!("if={}", image_path.to_string_lossy()))
                .arg(format!(
                    "of=/dev/{}",
                    if cfg!(target_os = "macos") {
                        format!("r{}", disk_info.id)
                    } else {
                        disk_info.id
                    }
                ))
                .spawn()
                .context("Spawning process for dd")?
        } else {
            anyhow::bail!("Windows not supported");
        };

        let exit_status = handle.wait().context("Executing dd")?;
        anyhow::ensure!(
            exit_status.success(),
            format!("dd exited with {}", exit_status)
        );

        log.info(format!(
            "Image '{}' flashed to drive '{}'",
            image_path.to_string_lossy(),
            disk_info_str
        ))?;
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

async fn run(log: &mut Logger) -> anyhow::Result<()> {
    match App::from_args() {
        App::Build(cmd) => cmd.run(log).await.context("Running build command")?,
        App::Deploy(cmd) => cmd.run(log).await.context("Running deploy command")?,
        App::Flash(cmd) => cmd.run(log).await.context("Running flash command")?,
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    let mut log = Logger::new();

    match run(&mut log).await {
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
