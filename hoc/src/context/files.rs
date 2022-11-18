use std::{borrow::Cow, fs::File as BlockingFile, future::Future, io, path::PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use tokio::fs::{self, File, OpenOptions};

use crate::{
    context::{
        key::{self, Key},
        Error,
    },
    prelude::*,
};

#[derive(Serialize, Deserialize)]
pub struct Files {
    #[serde(flatten)]
    map: IndexMap<PathBuf, PathBuf>,

    #[serde(skip)]
    pub(super) files_dir: PathBuf,
}

impl Files {
    pub(super) fn new<P>(files_dir: P) -> Self
    where
        P: Into<PathBuf>,
    {
        Self {
            map: IndexMap::new(),
            files_dir: files_dir.into(),
        }
    }

    #[throws(Error)]
    pub async fn create_file<'key, K, F, Fut>(
        &mut self,
        key: K,
        on_overwrite: F,
    ) -> (bool, (File, PathBuf))
    where
        K: Into<Cow<'key, Key>>,
        F: FnOnce(PathBuf) -> Fut,
        Fut: Future<Output = Result<(), Error>>,
    {
        let key = key.into();

        debug!("Create file for key: {key}");

        let mut file_options = OpenOptions::new();
        file_options.read(true).write(true);

        let mut had_previous_file = false;
        if let Some(path) = self.map.get(&**key) {
            error!("File for key {key} is already created");

            had_previous_file = true;
            let overwrite = select!("How do you want to resolve the key conflict?")
                .with_option("Skip", || false)
                .with_option("Overwrite", || true)
                .get()?;

            if !overwrite {
                warn!("Skipping to create file for key {key}");
                return (
                    had_previous_file,
                    (file_options.open(path).await?, path.clone()),
                );
            }

            warn!("Overwriting existing file for key {key}");
            file_options.truncate(true).create(true);

            on_overwrite(path.clone()).await?;
        } else {
            file_options.create_new(true);
        }

        let path = self.files_dir.join(&**key);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let file = match file_options.open(&path).await {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                error!("File at path {path:?} already exists");

                file_options.create_new(false);
                let overwrite = select!("How do you want to resolve the file path conflict?")
                    .with_option("Skip", || false)
                    .with_option("Overwrite", || true)
                    .get()?;

                if !overwrite {
                    warn!("Skipping to create file for key {key}");
                    file_options.open(&path).await?
                } else {
                    warn!("Overwriting existing file at path {path:?}");
                    file_options.truncate(true).open(&path).await?
                }
            }
            Err(err) => throw!(err),
        };

        self.map
            .insert(key.clone().into_owned().into_path_buf(), path.clone());

        (had_previous_file, (file, path))
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

        throw!(key::Error::KeyDoesNotExist(key.into_owned()));
    }

    #[throws(Error)]
    pub async fn remove_file<'key, K>(&mut self, key: K, force: bool)
    where
        K: Into<Cow<'key, Key>>,
    {
        let key = key.into();

        debug!("Remove file for key: {key}");

        match self.map.remove(&**key) {
            Some(path) => {
                fs::remove_file(path).await?;
            }
            None if !force => {
                error!("Key {key} does not exist.");

                let skipping = select!("How do you want to resolve the key conflict?")
                    .with_option("Skip", || true)
                    .get()?;

                if skipping {
                    warn!("Skipping to remove file for key {key}");
                }
            }
            None => (),
        }
    }
}

pub mod ledger {
    use std::{mem, path::PathBuf};

    use async_trait::async_trait;
    use tokio::fs;

    use crate::{
        context::{
            self,
            key::{self, KeyOwned},
        },
        ledger::Transaction,
        prelude::*,
    };

    pub struct Create {
        key: KeyOwned,
        current_file: PathBuf,
        previous_file: Option<PathBuf>,
    }

    impl Create {
        pub fn new(key: KeyOwned, current_file: PathBuf, previous_file: Option<PathBuf>) -> Self {
            Self {
                key,
                current_file,
                previous_file,
            }
        }
    }

    #[async_trait]
    impl Transaction for Create {
        fn description(&self) -> &'static str {
            "Create file"
        }

        async fn revert(&mut self) -> anyhow::Result<()> {
            let key = mem::replace(&mut self.key, key::KeyOwned::empty());
            let current_file = mem::take(&mut self.current_file);
            match self.previous_file.take() {
                Some(previous_file) => {
                    debug!("Move temporary file to persistent file location: {previous_file:?} => {current_file:?}");
                    fs::rename(previous_file, current_file).await?;
                }
                None => {
                    context::get_context()
                        .files_mut()
                        .await
                        .remove_file(key, true)
                        .await?;
                }
            }
            Ok(())
        }
    }
}
