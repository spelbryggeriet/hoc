use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::{
    context::{
        self,
        key::{Key, KeyOwned},
        Error,
    },
    prelude::*,
    util::Opt,
};

#[derive(Serialize, Deserialize)]
pub struct Cache {
    #[serde(flatten)]
    map: IndexMap<KeyOwned, PathBuf>,

    #[serde(skip, default = "Cache::cache_dir")]
    cache_dir: PathBuf,
}

impl Cache {
    pub(in crate::context) fn new() -> Self {
        Self {
            map: IndexMap::new(),
            cache_dir: Self::cache_dir(),
        }
    }

    fn cache_dir() -> PathBuf {
        crate::cache_dir().join("cache")
    }

    #[throws(Error)]
    pub fn get_or_create_file<K, C, O>(
        &mut self,
        key: K,
        on_cache: C,
        on_overwrite: O,
    ) -> (bool, (File, PathBuf))
    where
        K: Into<KeyOwned>,
        C: Fn(&mut File, &Path, bool) -> Result<(), Error>,
        O: FnOnce(&Path) -> Result<(), Error>,
    {
        let key = key.into();

        let mut had_previous_file = false;
        let mut file_options = File::options();
        file_options.write(true).read(true);

        if let Some(path) = self.map.get(&*key) {
            match file_options.open(path) {
                Ok(file) => {
                    had_previous_file = true;
                    debug!("Getting cached file: {key}");
                    return (had_previous_file, (file, path.clone()));
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => (),
                Err(err) => throw!(err),
            }
        }

        self.map.remove(&*key);

        file_options.create_new(true);

        let path = self.cache_dir.join(key.as_str());

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut should_overwrite = false;
        let mut file = match file_options.open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                error!("Cached file at path {path:?} already exists");

                had_previous_file = true;
                file_options.create_new(false);
                let opt = select!("How do you want to resolve the file path conflict?")
                    .with_options([Opt::Skip, Opt::Overwrite])
                    .get()?;

                should_overwrite = opt == Opt::Overwrite;
                if !should_overwrite {
                    debug!("Creating cached file: {key} (skipping)");
                    let file = file_options.open(&path)?;
                    self.map.insert(key, path.clone());
                    return (had_previous_file, (file, path));
                }

                on_overwrite(&path)?;
                file_options.truncate(true).open(&path)?
            }
            Err(err) => throw!(err),
        };

        context::util::cache_loop(&key, &mut file, &path, on_cache)?;

        if !should_overwrite {
            debug!("Creating cached file: {key}");
        } else {
            warn!("Creating cached file: {key} (overwriting)");
        }

        self.map.insert(key, path.clone());

        (had_previous_file, (file, path))
    }

    #[throws(Error)]
    pub fn _create_or_overwrite_file<K, C, O>(
        &mut self,
        key: K,
        on_cache: C,
        on_overwrite: O,
    ) -> (bool, (File, PathBuf))
    where
        K: Into<KeyOwned>,
        C: Fn(&mut File, &Path, bool) -> Result<(), Error>,
        O: FnOnce(&Path) -> Result<(), Error>,
    {
        let key = key.into();
        let path = self.cache_dir.join(key.as_str());

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut had_previous_file = false;
        let mut file_options = File::options();
        file_options
            .write(true)
            .read(true)
            .create(true)
            .truncate(true)
            .create_new(true);
        let mut file = match file_options.open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                had_previous_file = true;
                on_overwrite(&path)?;
                file_options.create_new(false).open(&path)?
            }
            Err(err) => throw!(err),
        };

        context::util::cache_loop(&key, &mut file, &path, on_cache)?;

        if !had_previous_file {
            debug!("Creating cached file: {key}");
        } else {
            debug!("Creating cached file: {key} (overwriting)");
        }

        self.map.insert(key, path.clone());

        (had_previous_file, (file, path))
    }

    #[throws(Error)]
    pub fn remove_file<K>(&mut self, key: &K, force: bool)
    where
        K: AsRef<Key> + ?Sized,
    {
        let key = key.as_ref();

        match self.map.remove(key) {
            Some(path) => {
                debug!("Remove cached file: {key}");
                match fs::remove_file(path) {
                    Ok(()) => (),
                    Err(err) if force && err.kind() == io::ErrorKind::NotFound => (),
                    Err(err) => throw!(err),
                };
            }
            None if !force => {
                error!("Key {key} does not exist.");

                select!("How do you want to resolve the key conflict?")
                    .with_option(Opt::Skip)
                    .get()?;

                warn!("Remove cached file: {key} (skipping)");
            }
            None => debug!("Remove cached file: {key} (skipping)"),
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
            "Create cached file".into()
        }

        fn detail(&self) -> Cow<'static, str> {
            format!("File to revert: {:?}", self.current_file).into()
        }

        #[throws(anyhow::Error)]
        fn revert(mut self: Box<Self>) {
            let current_file = self.current_file;
            match self.previous_file.take() {
                Some(previous_file) => {
                    debug!("Move temporary file: {previous_file:?} => {current_file:?}");
                    fs::rename(previous_file, current_file)?;
                }
                None => {
                    Context::get_or_init()
                        .cache_mut()
                        .remove_file(&self.key, true)?;
                }
            }
        }
    }
}
