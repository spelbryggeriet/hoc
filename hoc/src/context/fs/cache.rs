use std::{borrow::Cow, io::SeekFrom, path::PathBuf};

use futures::Future;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{self, AsyncSeekExt},
};

use crate::{
    context::{
        self,
        key::{Key, KeyOwned},
        CachedFileFn, Error,
    },
    prelude::*,
};

#[derive(Serialize, Deserialize)]
pub struct Cache {
    #[serde(flatten)]
    map: IndexMap<KeyOwned, PathBuf>,

    #[serde(skip)]
    pub(in crate::context) cache_dir: PathBuf,
}

impl Cache {
    pub(in crate::context) fn new<P>(cache_dir: P) -> Self
    where
        P: Into<PathBuf>,
    {
        Self {
            map: IndexMap::new(),
            cache_dir: cache_dir.into(),
        }
    }

    #[throws(Error)]
    pub async fn get_or_create_file<'key, K, F, O, Fut>(
        &mut self,
        key: K,
        cacher: F,
        on_overwrite: O,
    ) -> (bool, (File, PathBuf))
    where
        K: Into<Cow<'key, Key>>,
        F: for<'a> CachedFileFn<'a>,
        O: FnOnce(PathBuf) -> Fut,
        Fut: Future<Output = Result<(), Error>>,
    {
        let key = key.into();

        debug!("Create cached file for key: {key}");

        let mut had_previous_file = false;
        let mut file_options = OpenOptions::new();
        file_options.write(true).read(true);

        if let Some(path) = self.map.get(&*key) {
            match file_options.open(path).await {
                Ok(file) => {
                    had_previous_file = true;
                    return (had_previous_file, (file, path.clone()));
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => (),
                Err(err) => throw!(err),
            }
        }

        self.map.remove(&*key);

        file_options.create_new(true);

        let path = self.cache_dir.join(&*key.to_string_lossy());

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let mut file = match file_options.open(&path).await {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                error!("Cached file at path {path:?} already exists");

                had_previous_file = true;
                file_options.create_new(false);
                let should_overwrite = context::util::already_exists_prompt()?;

                if !should_overwrite {
                    warn!("Skipping to create cached file for key {key}");
                    let file = file_options.open(&path).await?;
                    self.map.insert(key.into_owned(), path.clone());

                    return (had_previous_file, (file, path));
                }

                warn!("Overwriting existing cached file at path {path:?}");
                on_overwrite(path.clone()).await?;
                file_options.truncate(true).open(&path).await?
            }
            Err(err) => throw!(err),
        };

        let caching_progress = progress_with_handle!("Caching file for key {:}", key);

        let mut retrying = false;
        loop {
            if let Err(err) = cacher(&mut file, &path, retrying).await {
                let custom_err = err.into();
                error!("{custom_err}");
                retrying = context::util::retry_prompt()?;
            } else {
                break;
            };

            if retrying {
                file.set_len(0).await?;
                file.seek(SeekFrom::Start(0)).await?;
            }
        }

        caching_progress.finish();

        self.map.insert(key.into_owned(), path.clone());

        (had_previous_file, (file, path))
    }

    #[throws(Error)]
    pub async fn _create_or_overwrite_file<'key, K, F, O, Fut>(
        &mut self,
        key: K,
        cacher: F,
        on_overwrite: O,
    ) -> (bool, (File, PathBuf))
    where
        K: Into<Cow<'key, Key>>,
        F: for<'a> CachedFileFn<'a>,
        O: FnOnce(PathBuf) -> Fut,
        Fut: Future<Output = Result<(), Error>>,
    {
        let key = key.into();

        debug!("Create cached file for key: {key}");

        let path = self.cache_dir.join(&*key.to_string_lossy());

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let mut had_previous_file = false;
        let mut file_options = OpenOptions::new();
        file_options
            .write(true)
            .read(true)
            .create(true)
            .truncate(true)
            .create_new(true);
        let mut file = match file_options.open(&path).await {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                had_previous_file = true;
                on_overwrite(path.clone()).await?;
                file_options.create_new(false).open(&path).await?
            }
            Err(err) => throw!(err),
        };

        let caching_progress = progress_with_handle!("Caching file for key {:}", key);

        let mut retrying = false;
        loop {
            if let Err(err) = cacher(&mut file, &path, retrying).await {
                let custom_err = err.into();
                error!("{custom_err}");
                retrying = context::util::retry_prompt()?;
            } else {
                break;
            };

            if retrying {
                file.set_len(0).await?;
                file.seek(SeekFrom::Start(0)).await?;
            }
        }

        caching_progress.finish();

        self.map.insert(key.into_owned(), path.clone());

        (had_previous_file, (file, path))
    }

    #[throws(Error)]
    pub async fn remove_file<'key, K>(&mut self, key: K, force: bool)
    where
        K: Into<Cow<'key, Key>>,
    {
        let key = key.into();

        debug!("Remove cached file for key: {key}");

        match self.map.remove(&*key) {
            Some(path) => {
                match fs::remove_file(path).await {
                    Ok(()) => (),
                    Err(err) if force && err.kind() == io::ErrorKind::NotFound => (),
                    Err(err) => throw!(err),
                };
            }
            None if !force => {
                error!("Key {key} does not exist.");

                let skipping = select!("How do you want to resolve the key conflict?")
                    .with_option("Skip", || true)
                    .get()?;

                if skipping {
                    warn!("Skipping to remove cached file for key {key}");
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
            "Create cached file"
        }

        async fn revert(&mut self) -> anyhow::Result<()> {
            let key = mem::replace(&mut self.key, key::KeyOwned::empty());
            let current_file = mem::take(&mut self.current_file);
            match self.previous_file.take() {
                Some(previous_file) => {
                    debug!("Move temporary file to cache file location: {previous_file:?} => {current_file:?}");
                    fs::rename(previous_file, current_file).await?;
                }
                None => {
                    context::get_context()
                        .cache_mut()
                        .await
                        .remove_file(key, true)
                        .await?;
                }
            }
            Ok(())
        }
    }
}
