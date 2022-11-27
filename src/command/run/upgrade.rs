use anyhow::Error;

use crate::prelude::*;

#[throws(Error)]
pub async fn run(from_ref: Option<String>) {
    if let Some(from_ref) = from_ref {
        compile_from_source(from_ref).await?;
    }
}

#[throws(Error)]
async fn compile_from_source(from_ref: String) {
    debug!("Creating cache directory");
    tokio::fs::create_dir_all(crate::cache_dir().join("source")).await?;
}
