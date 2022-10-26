use std::{borrow::Cow, path::PathBuf};

use async_std::{
    fs::{self, File as AsyncFile, OpenOptions},
    io,
    path::PathBuf as AsyncPathBuf,
};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;
use thiserror::Error;

use crate::{
    context::{
        key::{self, Key},
        Action,
    },
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
    pub async fn get_or_create_file_with<'key, K, F, E>(
        &mut self,
        key: K,
        f: F,
    ) -> (AsyncFile, AsyncPathBuf)
    where
        K: Into<Cow<'key, Key>>,
        F: for<'a> CachedFileFnOnce<'a, E>,
        E: Into<anyhow::Error> + 'static,
    {
        let key = key.into();

        debug!("Creating cached file for key: {key}");

        let mut file_options = OpenOptions::new();
        file_options.write(true).read(true);

        if let Some(path) = self.map.get(&**key) {
            match file_options.open(path).await {
                Ok(file) => return (file, AsyncPathBuf::from(path)),
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

                let option = select!("How do you want to resolve the file path conflict?")
                    .with_options(Action::iter())
                    .get()?;

                match option {
                    Action::Abort => throw!(err),
                    Action::Skip => {
                        warn!("Skipping to create file for key {key}");
                        file_options.create_new(false).open(&path).await?
                    }
                    Action::Overwrite => {
                        warn!("Overwriting existing file at path {path:?}");
                        file_options
                            .create_new(false)
                            .truncate(true)
                            .open(&path)
                            .await?
                    }
                }
            }
            Err(err) => throw!(err),
        };

        f(&mut file)
            .await
            .map_err(|err| Error::Custom(err.into()))?;

        self.map
            .insert(key.clone().into_owned().into_path_buf(), path);

        (file, AsyncPathBuf::from(key.into_owned().into_path_buf()))
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
