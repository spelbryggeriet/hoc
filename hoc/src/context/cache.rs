use std::{borrow::Cow, io::SeekFrom, path::PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{self, AsyncSeekExt},
};

use crate::{
    context::{key::Key, Error},
    prelude::*,
};

use super::CachedFileFn;

#[derive(Serialize, Deserialize)]
pub struct Cache {
    #[serde(flatten)]
    map: IndexMap<PathBuf, PathBuf>,

    #[serde(skip)]
    pub(super) cache_dir: PathBuf,
}

impl Cache {
    pub(super) fn new<P>(cache_dir: P) -> Self
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
        F: for<'a> CachedFileFn<'a>,
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

        self.map.remove(&**key);

        file_options.create_new(true);

        let path = self.cache_dir.join(&**key);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let mut file = match file_options.open(&path).await {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                error!("File at path {path:?} already exists");

                let skipping = select!("How do you want to resolve the file path conflict?")
                    .with_option("Skip", || {
                        warn!("Skipping to create file for key {key}");
                        true
                    })
                    .with_option("Overwrite", || {
                        warn!("Overwriting existing file at path {path:?}");
                        false
                    })
                    .get()?;

                let file = file_options
                    .create_new(false)
                    .truncate(!skipping)
                    .open(&path)
                    .await?;

                if skipping {
                    self.map
                        .insert(key.into_owned().into_path_buf(), path.clone());

                    return (file, path);
                }

                file
            }
            Err(err) => throw!(err),
        };

        let caching_progress = progress_with_handle!("Caching file {:?}", &**key);

        let mut retrying = false;
        loop {
            if let Err(err) = f(&mut file, &path, retrying).await {
                let custom_err = err.into();
                error!("{custom_err}");

                retrying = false;
                select!("How do you want to resolve the error?")
                    .with_option("Retry", || retrying = true)
                    .get()?;
            } else {
                break;
            };

            if retrying {
                file.set_len(0).await?;
                file.seek(SeekFrom::Start(0)).await?;
            }
        }

        caching_progress.finish();

        self.map
            .insert(key.into_owned().into_path_buf(), path.clone());

        (file, path)
    }

    #[throws(Error)]
    pub async fn _create_or_overwrite_file_with<K, F>(&mut self, key: K, f: F) -> (File, PathBuf)
    where
        K: Into<Cow<'static, Key>>,
        F: for<'a> CachedFileFn<'a>,
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
            .map_err(|err| Error::_Custom(err.into()))?;

        self.map
            .insert(key.into_owned().into_path_buf(), path.clone());

        (file, path)
    }
}
