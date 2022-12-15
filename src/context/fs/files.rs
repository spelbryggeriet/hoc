use std::{
    borrow::Cow,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::{
    context::{
        key::{self, Key, KeyOwned},
        Error,
    },
    prelude::*,
    util::Opt,
};

#[derive(Serialize, Deserialize)]
pub struct Files {
    #[serde(flatten)]
    map: IndexMap<KeyOwned, PathBuf>,

    #[serde(skip, default = "Files::files_dir")]
    files_dir: PathBuf,
}

impl Files {
    pub(in crate::context) fn new() -> Self {
        Self {
            map: IndexMap::new(),
            files_dir: Self::files_dir(),
        }
    }

    fn files_dir() -> PathBuf {
        crate::data_dir().join("files")
    }

    #[throws(Error)]
    pub fn create_file<K, F>(&mut self, key: K, on_overwrite: F) -> (bool, (File, PathBuf))
    where
        K: Into<KeyOwned>,
        F: FnOnce(&Path) -> Result<(), Error>,
    {
        let key = key.into();

        let mut file_options = File::options();
        file_options.read(true).write(true);

        let mut had_previous_file = false;
        let mut should_overwrite = false;
        if let Some(path) = self.map.get(&*key) {
            error!("File for key {key} is already created");

            had_previous_file = true;
            let opt = select!("How do you want to resolve the key conflict?")
                .with_options([Opt::Skip, Opt::Overwrite])
                .get()?;

            should_overwrite = opt == Opt::Overwrite;
            if !should_overwrite {
                warn!("Creating file: {key} (skipping)");
                return (had_previous_file, (file_options.open(path)?, path.clone()));
            }

            file_options.truncate(true).create(true);
            on_overwrite(&path)?;
        } else {
            file_options.create_new(true);
        }

        let path = self.files_dir.join(key.as_str());

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let file = match file_options.open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                error!("File at path {path:?} already exists");

                file_options.create_new(false);
                let opt = select!("How do you want to resolve the file path conflict?")
                    .with_options([Opt::Skip, Opt::Overwrite])
                    .get()?;

                should_overwrite = opt == Opt::Overwrite;
                if !should_overwrite {
                    warn!("Creating file: {key} (skipping)");
                    return (had_previous_file, (file_options.open(&path)?, path));
                } else {
                    file_options.truncate(true).open(&path)?
                }
            }
            Err(err) => throw!(err),
        };

        if !should_overwrite {
            debug!("Creating file: {key}");
        } else {
            warn!("Creating file: {key} (overwriting)");
        }

        self.map.insert(key, path.clone());

        (had_previous_file, (file, path))
    }

    #[throws(Error)]
    pub fn get_file<'key, K>(&self, key: K) -> (File, PathBuf)
    where
        K: Into<Cow<'key, Key>>,
    {
        let key = key.into();

        debug!("Getting file for key: {key}");

        let mut file_options = File::options();
        file_options.write(true).read(true);

        if let Some(path) = self.map.get(&*key) {
            let file = file_options.open(path)?;
            return (file, PathBuf::from(path));
        }

        throw!(key::Error::KeyDoesNotExist(key.into_owned()));
    }

    #[throws(Error)]
    pub fn remove_file<K>(&mut self, key: &K, force: bool)
    where
        K: AsRef<Key> + ?Sized,
    {
        let key = key.as_ref();

        match self.map.remove(key) {
            Some(path) => {
                debug!("Remove file: {key}");
                fs::remove_file(path)?;
            }
            None if !force => {
                error!("Key {key} does not exist.");

                select!("How do you want to resolve the key conflict?")
                    .with_option(Opt::Skip)
                    .get()?;

                warn!("Remove file: {key} (skipping)");
            }
            None => (),
        }
    }
}

pub mod ledger {
    use std::{borrow::Cow, fs, path::PathBuf};

    use crate::{
        context::{key::KeyOwned, Context},
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

    impl Transaction for Create {
        fn description(&self) -> Cow<'static, str> {
            "Create file".into()
        }

        fn detail(&self) -> Cow<'static, str> {
            format!("File to revert: {:?}", self.current_file).into()
        }

        fn revert(mut self: Box<Self>) -> anyhow::Result<()> {
            let current_file = self.current_file;
            match self.previous_file.take() {
                Some(previous_file) => {
                    debug!("Move temporary file: {previous_file:?} => {current_file:?}");
                    fs::rename(previous_file, current_file)?;
                }
                None => {
                    Context::get_or_init()
                        .files_mut()
                        .remove_file(&self.key, true)?;
                }
            }
            Ok(())
        }
    }
}
