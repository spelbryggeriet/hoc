use std::{borrow::Cow, path::PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    fs::{self, File, OpenOptions},
    io,
};

use crate::{
    context::key::{self, Key},
    prelude::*,
    prompt,
};

use super::CachedFileFnOnce;

#[derive(Serialize, Deserialize)]
pub struct Cache {
    #[serde(flatten)]
    map: IndexMap<PathBuf, PathBuf>,

    #[serde(skip)]
    pub(super) cache_dir: PathBuf,
}

impl Cache {
    pub fn new<P>(cache_dir: P) -> Self
    where
        P: Into<PathBuf>,
    {
        Self {
            map: IndexMap::new(),
            cache_dir: cache_dir.into(),
        }
    }

    #[throws(Error)]
    pub async fn get_or_create_file_with<K, F>(&mut self, key: K, f: F) -> (File, PathBuf)
    where
        K: Into<Cow<'static, Key>>,
        F: for<'a> CachedFileFnOnce<'a>,
    {
        let key = key.into();

        debug!("Creating cached file for key: {key}");

        let mut file_options = OpenOptions::new();
        file_options.write(true).read(true);

        if let Some(path) = self.map.get(&**key) {
            match file_options.open(path).await {
                Ok(file) => return (file, path.clone()),
                Err(err) if err.kind() == io::ErrorKind::NotFound => (),
                Err(err) => throw!(err),
            }
        }

        file_options.create_new(true);

        let path = self.cache_dir.join(&**key);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let mut file = match file_options.open(&path).await {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                error!("File at path {path:?} already exists");

                let should_truncate = select!("How do you want to resolve the file path conflict?")
                    .with_option("Abort", || Err(err))
                    .with_option("Skip", || {
                        warn!("Skipping to create file for key {key}");
                        Ok(false)
                    })
                    .with_option("Overwrite", || {
                        warn!("Overwriting existing file at path {path:?}");
                        Ok(true)
                    })
                    .get()??;

                file_options
                    .create_new(false)
                    .truncate(should_truncate)
                    .open(&path)
                    .await?
            }
            Err(err) => throw!(err),
        };

        f(&mut file, &path, false)
            .await
            .map_err(|err| Error::Custom(err.into()))?;

        self.map
            .insert(key.into_owned().into_path_buf(), path.clone());

        (file, path)
    }

    #[throws(Error)]
    pub async fn create_or_overwrite_file_with<K, F>(&mut self, key: K, f: F) -> (File, PathBuf)
    where
        K: Into<Cow<'static, Key>>,
        F: for<'a> CachedFileFnOnce<'a>,
    {
        let key = key.into();

        debug!("Creating cached file for key: {key}");

        let path = self.cache_dir.join(&**key);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let mut file = OpenOptions::new()
            .write(true)
            .read(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .await?;

        f(&mut file, &path, false)
            .await
            .map_err(|err| Error::Custom(err.into()))?;

        self.map
            .insert(key.into_owned().into_path_buf(), path.clone());

        (file, path)
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),

    #[error(transparent)]
    Key(#[from] key::Error),

    #[error(transparent)]
    Prompt(#[from] prompt::Error),

    #[error(transparent)]
    Custom(anyhow::Error),
}
