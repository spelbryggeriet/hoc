use std::{
    borrow::Cow,
    fs::{self, File as BlockingFile},
    io,
    path::PathBuf,
};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::fs::File;

use crate::{
    context::key::{self, Key},
    prelude::*,
    prompt,
};

#[derive(Serialize, Deserialize)]
pub struct Files {
    #[serde(flatten)]
    map: IndexMap<PathBuf, PathBuf>,

    #[serde(skip)]
    pub(super) files_dir: PathBuf,
}

impl Files {
    pub fn new<P>(files_dir: P) -> Self
    where
        P: Into<PathBuf>,
    {
        Self {
            map: IndexMap::new(),
            files_dir: files_dir.into(),
        }
    }

    #[throws(Error)]
    pub fn create_file<'key, K>(&mut self, key: K) -> (File, PathBuf)
    where
        K: Into<Cow<'key, Key>>,
    {
        let key = key.into();

        debug!("Create file for key: {key}");

        let mut file_options = BlockingFile::options();
        file_options.write(true).read(true);

        if let Some(path) = self.map.get(&**key) {
            error!("File for key {key} is already created");

            let file_options_clone = file_options.clone();
            let file_pair = select!("How do you want to resolve the key conflict?")
                .with_option("Skip", || -> Result<_, Error> {
                    warn!("Skipping to create file for key {key}");
                    Ok(Some((File::from_std(file_options_clone.open(path)?), path)))
                })
                .with_option("Overwrite", || {
                    warn!("Overwriting existing file for key {key}");
                    file_options.truncate(true);
                    Ok(None)
                })
                .get()??;

            if let Some((file, path)) = file_pair {
                return (file, path.clone());
            }
        } else {
            file_options.create_new(true);
        }

        let path = self.files_dir.join(&**key);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let file = match file_options.open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                error!("File at path {path:?} already exists");

                let mut file_options_clone = file_options.clone();
                select!("How do you want to resolve the file path conflict?")
                    .with_option("Skip", || -> Result<_, Error> {
                        warn!("Skipping to create file for key {key}");
                        Ok(file_options_clone.create_new(false).open(&path)?)
                    })
                    .with_option("Overwrite", || {
                        warn!("Overwriting existing file at path {path:?}");
                        Ok(file_options.create_new(false).truncate(true).open(&path)?)
                    })
                    .get()??
            }
            Err(err) => throw!(err),
        };

        self.map
            .insert(key.clone().into_owned().into_path_buf(), path);

        (File::from_std(file), key.into_owned().into_path_buf())
    }

    #[throws(Error)]
    pub fn get_file<'key, K>(&self, key: K) -> (File, PathBuf)
    where
        K: Into<Cow<'key, Key>>,
    {
        let key = key.into();

        debug!("Get file for key: {key}");

        let mut file_options = BlockingFile::options();
        file_options.write(true).read(true);

        if let Some(path) = self.map.get(&**key) {
            let file = match file_options.open(path) {
                Ok(file) => file,
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    error!("File at path {path:?} not found");

                    return select!("How do you want to resolve the file path conflict?").get()?;
                }
                Err(err) => throw!(err),
            };

            return (File::from_std(file), PathBuf::from(path));
        }

        error!("File for key {key} does not exists");

        return select!("How do you want to resolve the key conflict?").get()?;
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
}
