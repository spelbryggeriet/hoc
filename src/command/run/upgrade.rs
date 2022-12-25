use std::{
    borrow::Cow,
    env,
    fs::File,
    io::{self, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use anyhow::Error;
use crossterm::style::Stylize;
use reqwest::{
    blocking::Client,
    header::{HeaderMap, HeaderValue, USER_AGENT},
};
use serde::Deserialize;
use zip::ZipArchive;

use crate::{
    context::fs::{temp, ContextFile},
    prelude::*,
};

const DEFAULT_REPO_URL: &str = "https://github.com/spelbryggeriet/hoc.git";
const EXECUTABLE_HOME_DESTINATION_PATH: &str = ".local/bin/hoc";

const GITHUB_API_LATEST: &str = "https://api.github.com/repos/spelbryggeriet/hoc/releases/latest";
const GITHUB_API_RELEASE_DOWNLOAD_TEMPLATE: &str = "https://github.com/spelbryggeriet/hoc/releases/download/#version/hoc_macos-x86_64_#version.zip";

#[throws(Error)]
pub fn run(from_ref: Option<String>) {
    let version = if let Some(from_ref) = from_ref {
        compile_from_source(&from_ref)?;
        from_ref
    } else if let Some(latest_version) = download_latest()? {
        latest_version
    } else {
        return;
    };

    info!("{} at {version} installed", "hoc".yellow());
}

#[throws(Error)]
fn compile_from_source(from_ref: &str) {
    let source_path = get_source_path();
    if !git_path_exists(&source_path)? {
        fetch_source(&source_path)?;
    }

    checkout_ref(&source_path, from_ref)?;
    let executable_path = build(&source_path)?;
    install_by_path(executable_path)?;
}

#[throws(Error)]
fn download_latest() -> Option<String> {
    let client = get_github_client()?;
    let latest_version = determine_latest_version(&client)?;

    debug!("Found latest version: {latest_version}");

    if latest_version.trim_start_matches('v') == env!("CARGO_PKG_VERSION") {
        info!("{} is up to date", "hoc".yellow());
        return None;
    }

    let file = download(&client, &latest_version)?;
    install_by_file(file)?;

    Some(latest_version)
}

fn get_source_path() -> PathBuf {
    crate::local_cache_dir().join("source")
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
fn fetch_source(dest_path: &Path) {
    progress!("Fetching source");

    let repo_url = env::var("HOC_REPO")
        .map(Cow::Owned)
        .unwrap_or(Cow::Borrowed(DEFAULT_REPO_URL));

    info!("Repository URL: {repo_url}");

    process!("git clone --depth=1 {repo_url} {dest_path:?}").run()?;
}

#[throws(Error)]
fn checkout_ref(source_path: &Path, from_ref: &str) {
    progress!("Checking out source");

    let source_path_string = source_path.to_string_lossy().into_owned();

    process!("git fetch --force origin {from_ref}")
        .current_dir(source_path_string.clone())
        .run()?;
    process!("git reset --hard FETCH_HEAD")
        .current_dir(source_path_string)
        .run()?;
}

#[throws(Error)]
fn build(source_path: &Path) -> PathBuf {
    progress!("Building");

    process!("cargo build --release")
        .current_dir(source_path.to_string_lossy().into_owned())
        .run()?;

    source_path.join("target/release/hoc")
}

#[throws(Error)]
fn install_by_path(executable_source_path: PathBuf) {
    progress!("Installing");

    let executable_destination_path = get_executable_destination_path();

    process!("cp {executable_source_path:?} {executable_destination_path:?}").run()?;
}

#[throws(Error)]
fn get_github_client() -> Client {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("hoc"));
    Client::builder().default_headers(headers).build()?
}

#[throws(Error)]
fn determine_latest_version(client: &Client) -> String {
    #[derive(Deserialize)]
    struct Latest {
        tag_name: String,
    }

    let latest: Latest = client.get(GITHUB_API_LATEST).send()?.json()?;

    latest.tag_name
}

#[throws(Error)]
fn download(client: &Client, version: &str) -> ContextFile {
    progress!("Downloading {version}");

    let mut file = temp::create_file()?;

    client
        .get(GITHUB_API_RELEASE_DOWNLOAD_TEMPLATE.replace("#version", version))
        .send()?
        .copy_to(&mut file)?;

    file.seek(SeekFrom::Start(0))?;
    file
}

#[throws(Error)]
fn install_by_file(file: ContextFile) {
    progress!("Installing");

    let mut archive = ZipArchive::new(file)?;
    let mut executable_file = archive.by_name("hoc")?;

    let mut destination_file = File::options()
        .create(true)
        .write(true)
        .open(get_executable_destination_path())?;

    io::copy(&mut executable_file, &mut destination_file)?;
}
