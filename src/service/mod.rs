pub mod ci;

use std::path::Path;
use std::env;

use git2::{Cred, RemoteCallbacks, Repository};

use crate::prelude::*;

pub const REGISTRY_DOMAIN: &str = "registry.gitlab.com";
const PROJECT_NAME: &str = "lidin-homepi";

pub fn image_name(service: &str) -> String {
    format!("{}/{}/{}", REGISTRY_DOMAIN, PROJECT_NAME, service)
}

pub fn full_image_name(service: &str, tag: &str) -> String {
    format!("{}/{}/{}:{}", REGISTRY_DOMAIN, PROJECT_NAME, service, tag)
}

pub fn clone_repo(service: &str, repo_path: &Path) -> AppResult<Repository> {
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
    let url = format!("git@gitlab.com:lidin-homepi/{}.git", service);
    status!(
        "Cloning repository '{}' into directory '{}'",
        &url,
        repo_path.to_string_lossy()
    );
    builder
        .clone(&url, repo_path)
        .context(format!("Cloning repository '{}'", &url))
}
