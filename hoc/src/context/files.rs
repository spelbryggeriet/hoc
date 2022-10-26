use std::{
    borrow::Cow,
    fs::{self, File},
    io,
    path::PathBuf,
};

use async_std::{fs::File as AsyncFile, path::PathBuf as AsyncPathBuf};
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
    pub fn create_file<'key, K>(&mut self, key: K) -> (AsyncFile, AsyncPathBuf)
    where
        K: Into<Cow<'key, Key>>,
    {
        let key = key.into();

        debug!("Create file for key: {key}");

        let mut file_options = File::options();
        file_options.write(true).read(true);

        if let Some(path) = self.map.get(&**key) {
            error!("File for key {key} is already created");

            let option = select!("How do you want to resolve the key conflict?")
                .with_options(Action::iter())
                .get()?;

            match option {
                Action::Abort => {
                    throw!(Error::Key(key::Error::KeyAlreadyExists(key.into_owned())));
                }
                Action::Skip => {
                    warn!("Skipping to create file for key {key}");
                    return (
                        AsyncFile::from(file_options.open(path)?),
                        AsyncPathBuf::from(path),
                    );
                }
                Action::Overwrite => {
                    warn!("Overwriting existing file for key {key}");
                    file_options.truncate(true);
                }
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

                let option = select!("How do you want to resolve the file path conflict?")
                    .with_options(Action::iter())
                    .get()?;

                match option {
                    Action::Abort => {
                        throw!(Error::Io(io::ErrorKind::AlreadyExists.into()));
                    }
                    Action::Skip => {
                        warn!("Skipping to create file for key {key}");
                        file_options.create_new(false).open(&path)?
                    }
                    Action::Overwrite => {
                        warn!("Overwriting existing file at path {path:?}");
                        file_options.create_new(false).truncate(true).open(&path)?
                    }
                }
            }
            Err(err) => throw!(err),
        };

        self.map
            .insert(key.clone().into_owned().into_path_buf(), path);

        (
            AsyncFile::from(file),
            AsyncPathBuf::from(key.into_owned().into_path_buf()),
        )
    }

    #[throws(Error)]
    pub fn get_file<'key, K>(&self, key: K) -> (AsyncFile, AsyncPathBuf)
    where
        K: Into<Cow<'key, Key>>,
    {
        let key = key.into();

        debug!("Get file for key: {key}");

        let mut file_options = File::options();
        file_options.write(true).read(true);

        if let Some(path) = self.map.get(&**key) {
            let file = match file_options.open(path) {
                Ok(file) => file,
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    error!("File at path {path:?} not found");

                    let option = select!("How do you want to resolve the file path conflict?")
                        .with_option(Action::Abort)
                        .get()?;

                    match option {
                        Action::Abort => throw!(Error::Io(io::ErrorKind::NotFound.into())),
                        _ => unreachable!(),
                    }
                }
                Err(err) => throw!(err),
            };

            return (AsyncFile::from(file), AsyncPathBuf::from(path));
        }

        error!("File for key {key} does not exists");

        let _option = select!("How do you want to resolve the key conflict?")
            .with_option(Action::Abort)
            .get()?;

        throw!(Error::Key(key::Error::KeyDoesNotExist(key.into_owned())));
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
