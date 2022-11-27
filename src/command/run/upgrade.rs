use std::{
    borrow::Cow,
    env,
    path::{Path, PathBuf},
};

use anyhow::Error;

use crate::prelude::*;

const DEFAULT_REPO_URL: &str = "https://github.com/spelbryggeriet/hoc.git";
const EXECUTABLE_HOME_DESTINATION: &str = ".local/bin/hoc";

#[throws(Error)]
pub async fn run(from_ref: Option<String>) {
    if let Some(from_ref) = from_ref {
        compile_from_source(from_ref).await?;
    }
}

#[throws(Error)]
async fn compile_from_source(from_ref: String) {
    let source_path = get_source_path();
    if !git_path_exists(&source_path)? {
        fetch_source(&source_path).await?;
    }

    checkout_ref(&source_path, from_ref).await?;
    let executable_path = build(&source_path).await?;
    install(executable_path).await?;
}

fn get_source_path() -> PathBuf {
    crate::cache_dir().join("source")
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
async fn checkout_ref(source_path: &Path, from_ref: String) {
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
async fn install(executable_path: PathBuf) {
    progress!("Installing");

    let home_dir = env::var("HOME").expect(EXPECT_HOME_ENV_VAR);
    let executable_destination = PathBuf::from(home_dir).join(EXECUTABLE_HOME_DESTINATION);

    cmd!("cp {executable_path:?} {executable_destination:?}").await?;
}
