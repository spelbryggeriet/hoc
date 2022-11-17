use std::{
    borrow::Cow,
    fs::{self as blocking_fs, File as BlockingFile},
    io,
    path::PathBuf,
};

use indexmap::IndexMap;
use tokio::{
    fs::{self, File},
    runtime::Handle,
    task,
};

use crate::{
    context::{key::Key, Error},
    prelude::*,
    util,
};

pub struct TempFiles {
    map: IndexMap<PathBuf, PathBuf>,
    pub(super) files_dir: PathBuf,
}

impl TempFiles {
    const RAND_CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVXYZ\
                              abcdefghijklmnopqrstuvxyz\
                              0123456789";

    pub(super) fn new<P>(files_dir: P) -> Self
    where
        P: Into<PathBuf>,
    {
        Self {
            map: IndexMap::new(),
            files_dir: files_dir.into(),
        }
    }

    pub(super) fn empty() -> Self {
        Self {
            map: IndexMap::new(),
            files_dir: PathBuf::new(),
        }
    }

    #[throws(Error)]
    pub fn create_file<'key, K>(&mut self, key: K) -> (File, PathBuf)
    where
        K: Into<Cow<'key, Key>>,
    {
        let key = key.into();

        debug!("Create temporary file for key: {key}");

        let mut file_options = BlockingFile::options();
        file_options.write(true).truncate(true).read(true);

        if let Some(path) = self.map.get(&**key) {
            error!("Temporary file for key {key} is already created");

            let overwrite = select!("How do you want to resolve the key conflict?")
                .with_option("Skip", || false)
                .with_option("Overwrite", || true)
                .get()?;

            if !overwrite {
                warn!("Skipping to create temporary file for key {key}");
                let file = File::from_std(file_options.open(path)?);
                return (file, path.clone());
            }

            warn!("Overwriting existing temporary file for key {key}");
        }

        blocking_fs::create_dir_all(&self.files_dir)?;

        let mut path = self.files_dir.clone();
        let file = loop {
            path.push(util::random_string(Self::RAND_CHARS, 10));
            match file_options
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => break file,
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    path.pop();
                }
                Err(err) => throw!(err),
            };
        };

        self.map
            .insert(key.clone().into_owned().into_path_buf(), path.clone());

        (File::from_std(file), path)
    }

    #[throws(Error)]
    pub fn get_file<'key, K>(&self, key: K) -> (File, PathBuf)
    where
        K: Into<Cow<'key, Key>>,
    {
        let key = key.into();

        debug!("Get temporary file for key: {key}");

        let mut file_options = BlockingFile::options();
        file_options.write(true).read(true);

        if let Some(path) = self.map.get(&**key) {
            let file = match file_options.open(path) {
                Ok(file) => file,
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    error!("Temporary file at path {path:?} not found");

                    return select!("How do you want to resolve the file path conflict?").get()?;
                }
                Err(err) => throw!(err),
            };

            return (File::from_std(file), PathBuf::from(path));
        }

        error!("Temporary file for key {key} does not exists");

        return select!("How do you want to resolve the key conflict?").get()?;
    }

    #[throws(Error)]
    pub async fn remove_file<'key, K>(&mut self, key: K, force: bool)
    where
        K: Into<Cow<'key, Key>>,
    {
        let key = key.into();

        debug!("Remove temporary file for key: {key}");

        match self.map.remove(&**key) {
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
                    warn!("Skipping to remove temporary file for key {key}");
                }
            }
            None => (),
        }
    }

    #[throws(Error)]
    pub async fn clean(&self) {
        let mut read_dir = match fs::read_dir(&self.files_dir).await {
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
}

impl Drop for TempFiles {
    fn drop(&mut self) {
        let _ = task::block_in_place(|| Handle::current().block_on(self.clean()));
    }
}
