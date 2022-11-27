use std::{
    borrow::Cow,
    env,
    fs::File as BlockingFile,
    io::{self as blocking_io, SeekFrom},
    path::{Path, PathBuf},
};

use anyhow::Error;
use crossterm::style::Stylize;
use reqwest::{
    header::{HeaderMap, HeaderValue, USER_AGENT},
    Client,
};
use serde::Deserialize;
use tokio::{
    fs::File,
    io::{AsyncSeekExt, AsyncWriteExt},
};
use zip::ZipArchive;

use crate::{prelude::*, temp};

const DEFAULT_REPO_URL: &str = "https://github.com/spelbryggeriet/hoc.git";
const EXECUTABLE_HOME_DESTINATION_PATH: &str = ".local/bin/hoc";

const GITHUB_API_LATEST: &str = "https://api.github.com/repos/spelbryggeriet/hoc/releases/latest";
const GITHUB_API_RELEASE_DOWNLOAD_TEMPLATE: &str = "https://github.com/spelbryggeriet/hoc/releases/download/#version/hoc_macos-x86_64_#version.zip";

#[throws(Error)]
pub async fn run(from_ref: Option<String>) {
    let version = if let Some(from_ref) = from_ref {
        compile_from_source(&from_ref).await?;
        from_ref
    } else if let Some(latest_version) = download_latest().await? {
        latest_version
    } else {
        return;
    };

    info!("{} at {version} installed", "hoc".yellow());
}

#[throws(Error)]
async fn compile_from_source(from_ref: &str) {
    let source_path = get_source_path();
    if !git_path_exists(&source_path)? {
        fetch_source(&source_path).await?;
    }

    checkout_ref(&source_path, from_ref).await?;
    let executable_path = build(&source_path).await?;
    install_by_path(executable_path).await?;
}

#[throws(Error)]
async fn download_latest() -> Option<String> {
    let client = get_github_client()?;
    let latest_version = determine_latest_version(&client).await?;

    debug!("Found latest version: {latest_version}");

    if latest_version.trim_start_matches('v') == env!("CARGO_PKG_VERSION") {
        info!("{} is up to date", "hoc".yellow());
        return None;
    }

    let file = download(&client, &latest_version).await?;
    install_by_file(file).await?;

    Some(latest_version)
}

fn get_source_path() -> PathBuf {
    crate::cache_dir().join("source")
}

fn get_executable_destination_path() -> PathBuf {
    let home_dir = env::var("HOME").expect(EXPECT_HOME_ENV_VAR);
    PathBuf::from(home_dir).join(EXECUTABLE_HOME_DESTINATION_PATH)
}

#[throws(Error)]
fn git_path_exists(source_path: &Path) -> bool {
    source_path.join(".git").try_exists()?
}

#[throws(Error)]
async fn fetch_source(dest_path: &Path) {
    progress!("Fetching source");

    let repo_url = env::var("HOC_REPO")
        .map(Cow::Owned)
        .unwrap_or(Cow::Borrowed(DEFAULT_REPO_URL));

    info!("Repository URL: {repo_url}");

    cmd!("git clone {repo_url} {dest_path:?}")
        .revertible(cmd!("rm -fr {dest_path:?}"))
        .await?;
}

#[throws(Error)]
async fn checkout_ref(source_path: &Path, from_ref: &str) {
    progress!("Checking out source");

    let source_path_string = source_path.to_string_lossy().into_owned();

    let original_branch = cmd!("git branch --show-current")
        .current_dir(source_path_string.clone())
        .await?
        .stdout;
    cmd!("git checkout {from_ref}")
        .current_dir(source_path_string.clone())
        .revertible(cmd!("git checkout {original_branch}").current_dir(source_path_string.clone()))
        .await?
}

#[throws(Error)]
async fn build(source_path: &Path) -> PathBuf {
    progress!("Building");

    cmd!("cargo build --release")
        .current_dir(source_path.to_string_lossy().into_owned())
        .await?;

    source_path.join("target/release/hoc")
}

#[throws(Error)]
async fn install_by_path(executable_source_path: PathBuf) {
    progress!("Installing");

    let executable_destination_path = get_executable_destination_path();

    cmd!("cp {executable_source_path:?} {executable_destination_path:?}").await?;
}

#[throws(Error)]
fn get_github_client() -> Client {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("hoc"));
    Client::builder().default_headers(headers).build()?
}

#[throws(Error)]
async fn determine_latest_version(client: &Client) -> String {
    #[derive(Deserialize)]
    struct Latest {
        tag_name: String,
    }

    let latest: Latest = client.get(GITHUB_API_LATEST).send().await?.json().await?;

    latest.tag_name
}

#[throws(Error)]
async fn download(client: &Client, version: &str) -> File {
    progress!("Downloading {version}");

    let mut data = client
        .get(GITHUB_API_RELEASE_DOWNLOAD_TEMPLATE.replace("#version", version))
        .send()
        .await?;

    let (mut file, _) = temp::create_file().await?;

    while let Some(chunk) = data.chunk().await? {
        file.write_all(&chunk).await?;
    }

    file.seek(SeekFrom::Start(0)).await?;
    file
}

#[throws(Error)]
async fn install_by_file(file: File) {
    progress!("Installing");

    let mut archive = ZipArchive::new(file.into_std().await)?;
    let mut executable_file = archive.by_name("hoc")?;

    let mut destination_file = BlockingFile::options()
        .create(true)
        .write(true)
        .open(get_executable_destination_path())?;

    blocking_io::copy(&mut executable_file, &mut destination_file)?;
}
