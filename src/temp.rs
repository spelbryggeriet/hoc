use std::{io, path::PathBuf};

use tokio::fs::{self, File, OpenOptions};

use crate::{context::Error, prelude::*, util};

const RAND_CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVXYZ\
                              abcdefghijklmnopqrstuvxyz\
                              0123456789";

fn get_temp_dir() -> PathBuf {
    crate::cache_dir().join("temp")
}

#[throws(Error)]
pub async fn create_file() -> (File, PathBuf) {
    let mut file_options = OpenOptions::new();
    file_options.write(true).truncate(true).read(true);

    let mut path = get_temp_dir();
    fs::create_dir_all(&path).await?;

    let mut attempt = 1;
    let file = loop {
        path.push(util::random_string(RAND_CHARS, 10));
        if attempt == 1 {
            debug!("Creating temporary file: {path:?}");
        } else {
            warn!("Creating temporary file: {path:?} (attempt {attempt})");
        }

        match file_options
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(file) => break file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => path.pop(),
            Err(err) => throw!(err),
        };

        attempt += 1;
    };

    (file, path)
}

#[throws(Error)]
pub async fn clean() {
    let mut read_dir = match fs::read_dir(get_temp_dir()).await {
        Ok(read_dir) => read_dir,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return,
        Err(err) => throw!(err),
    };

    while let Some(entry) = read_dir.next_entry().await? {
        if entry.file_type().await?.is_file() {
            fs::remove_file(entry.path()).await?;
        }
    }
}
