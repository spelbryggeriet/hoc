use std::{
    borrow::Cow,
    convert::Infallible,
    fmt::Display,
    fs::File,
    io,
    marker::PhantomData,
    os::unix::fs::PermissionsExt,
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use once_cell::sync::OnceCell;
use serde::{ser::SerializeMap, Deserialize, Deserializer, Serialize};
use thiserror::Error;

use self::{
    fs::{cache::Cache, files::Files, temp::Temp},
    key::{Key, KeyOwned},
    kv::{Item, Kv, PutOptions, Value},
};
use crate::{ledger::Ledger, prelude::*, prompt};

pub mod fs;
pub mod key;
pub mod kv;
mod util;

#[throws(D::Error)]
fn deserialize_rw_lock<'de, D, T>(deserializer: D) -> RwLock<T>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    RwLock::new(T::deserialize(deserializer)?)
}

#[derive(Deserialize)]
pub struct Context {
    #[serde(deserialize_with = "deserialize_rw_lock")]
    kv: RwLock<Kv>,
    #[serde(deserialize_with = "deserialize_rw_lock")]
    files: RwLock<Files>,
    #[serde(deserialize_with = "deserialize_rw_lock")]
    cache: RwLock<Cache>,
    #[serde(skip)]
    temp: RwLock<Temp>,
}

impl Context {
    pub fn get_or_init() -> &'static Context {
        static CONTEXT: OnceCell<Context> = OnceCell::new();

        CONTEXT.get_or_init(Context::new)
    }

    fn new() -> Self {
        Self {
            kv: RwLock::new(Kv::new()),
            files: RwLock::new(Files::new()),
            cache: RwLock::new(Cache::new()),
            temp: RwLock::new(Temp::new()),
        }
    }

    #[throws(anyhow::Error)]
    pub fn load(&self) {
        debug!("Loading context");

        let context_path = crate::local_context_file_path();

        let files_dir = crate::local_files_dir();
        let cache_dir = crate::local_cache_dir();
        let temp_dir = crate::local_temp_dir();
        let source_dir = crate::local_source_dir();

        debug!("Creating files directory");
        std::fs::create_dir_all(&files_dir)?;

        debug!("Creating cache directory");
        std::fs::create_dir_all(&cache_dir)?;

        debug!("Creating temp directory");
        std::fs::create_dir_all(&temp_dir)?;

        debug!("Creating source directory");
        std::fs::create_dir_all(&source_dir)?;

        debug!("Setting files directory permissions");
        let mut permissions = files_dir.metadata()?.permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&files_dir, permissions)?;

        debug!("Setting cache directory permissions");
        let mut permissions = cache_dir.metadata()?.permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&cache_dir, permissions)?;

        debug!("Setting temp directory permissions");
        let mut permissions = temp_dir.metadata()?.permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&temp_dir, permissions)?;

        debug!("Setting source directory permissions");
        let mut permissions = source_dir.metadata()?.permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&source_dir, permissions)?;

        debug!("Opening context file");
        match File::options().read(true).write(true).open(&context_path) {
            Ok(file) => {
                debug!("Using pre-existing context file: {context_path:?}");

                debug!("Deserializing context from file");
                let context: Self = serde_yaml::from_reader(file)?;
                *self.kv_mut() = context.kv.into_inner().expect(EXPECT_THREAD_NOT_POSIONED);
                *self.files_mut() = context
                    .files
                    .into_inner()
                    .expect(EXPECT_THREAD_NOT_POSIONED);
                *self.cache_mut() = context
                    .cache
                    .into_inner()
                    .expect(EXPECT_THREAD_NOT_POSIONED);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                debug!("No context file found");
            }
            Err(error) => throw!(error),
        }
    }

    pub fn kv(&self) -> RwLockReadGuard<Kv> {
        self.kv.read().expect(EXPECT_THREAD_NOT_POSIONED)
    }

    pub fn kv_mut(&self) -> RwLockWriteGuard<Kv> {
        self.kv.write().expect(EXPECT_THREAD_NOT_POSIONED)
    }

    pub fn files(&self) -> RwLockReadGuard<Files> {
        self.files.read().expect(EXPECT_THREAD_NOT_POSIONED)
    }

    pub fn files_mut(&self) -> RwLockWriteGuard<Files> {
        self.files.write().expect(EXPECT_THREAD_NOT_POSIONED)
    }

    pub fn cache(&self) -> RwLockReadGuard<Cache> {
        self.cache.read().expect(EXPECT_THREAD_NOT_POSIONED)
    }

    pub fn cache_mut(&self) -> RwLockWriteGuard<Cache> {
        self.cache.write().expect(EXPECT_THREAD_NOT_POSIONED)
    }

    pub fn temp(&self) -> RwLockReadGuard<Temp> {
        self.temp.read().expect(EXPECT_THREAD_NOT_POSIONED)
    }

    #[throws(anyhow::Error)]
    pub fn persist(&self) {
        progress!("Persisting context");

        debug!("Dropping temporary values");
        self.kv_mut().drop_temporary_values();

        debug!("Opening context file for writing");
        let file = File::options()
            .write(true)
            .truncate(true)
            .create(true)
            .open(crate::local_context_file_path())?;

        debug!("Serializing context to file");
        serde_yaml::to_writer(file, self)?;
    }

    #[throws(anyhow::Error)]
    pub fn cleanup(&self) {
        debug!("Clean temporary files");
        self.temp().cleanup()?;
    }
}

impl Serialize for Context {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut context = serializer.serialize_map(Some(3))?;
        context.serialize_entry("kv", &*self.kv())?;
        context.serialize_entry("files", &*self.files())?;
        context.serialize_entry("cache", &*self.cache())?;
        context.end()
    }
}

pub struct KvBuilder<'a, O> {
    key: Cow<'a, Key>,
    temporary: bool,
    _operation: PhantomData<O>,
}

pub enum All {}
pub enum Put {}

impl<'a> KvBuilder<'a, All> {
    pub fn new(key: Cow<'a, Key>) -> Self {
        Self {
            key,
            temporary: false,
            _operation: Default::default(),
        }
    }

    pub fn temporary(self) -> KvBuilder<'a, Put> {
        KvBuilder {
            key: self.key,
            temporary: true,
            _operation: Default::default(),
        }
    }

    #[throws(Error)]
    pub fn get(self) -> Item {
        Context::get_or_init().kv().get_item(&self.key)?
    }

    #[allow(unused)]
    pub fn exists(self) -> bool {
        Context::get_or_init().kv().item_exists(&self.key)
    }

    #[throws(Error)]
    pub fn update<V>(self, value: V)
    where
        V: Into<Value> + Clone + Display + Send + 'static,
    {
        self.put_or_update(value, true)?;
    }

    #[allow(unused)]
    #[throws(Error)]
    pub fn drop(self) {
        let item =
            Context::get_or_init()
                .kv_mut()
                .drop_item(&self.key, |value, is_temporary| {
                    if is_temporary {
                        value.take();
                    }
                })?;

        if let Some(item) = item {
            Ledger::get_or_init().add(ledger::Drop::new(self.key.into_owned(), item));
        }
    }
}

impl<'a, O> KvBuilder<'a, O> {
    #[throws(Error)]
    fn put_or_update<V>(self, value: V, update: bool)
    where
        V: Into<Value> + Clone + Display + Send + 'static,
    {
        let previous_value = Context::get_or_init().kv_mut().put_value(
            &self.key,
            value.clone(),
            PutOptions {
                temporary: self.temporary,
                update,
            },
        )?;

        if !self.temporary && previous_value != Some(None) {
            Ledger::get_or_init().add(ledger::Put::new(
                self.key.into_owned(),
                value,
                previous_value.flatten().map(Secret::new),
            ));
        }
    }

    #[throws(Error)]
    pub fn put<V>(self, value: V)
    where
        V: Into<Value> + Clone + Display + Send + 'static,
    {
        self.put_or_update(value, false)?;
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("Key already exists: {0:?}")]
    KeyAlreadyExists(KeyOwned),

    #[error("Key does not exist: {0:?}")]
    KeyDoesNotExist(KeyOwned),

    #[error("Mismatched value types: expected {expected} ≠ actual {actual}")]
    MismatchedTypes {
        expected: kv::TypeDescription,
        actual: kv::TypeDescription,
    },

    #[error("{0} out of range for `{1}`")]
    OverflowingNumber(i128, &'static str),

    #[error(transparent)]
    Io(#[from] io::Error),

    #[error(transparent)]
    Prompt(#[from] prompt::Error),

    #[error(transparent)]
    Serialization(#[from] serde_json::Error),

    #[error(transparent)]
    Custom(#[from] anyhow::Error),
}

impl From<Infallible> for Error {
    fn from(x: Infallible) -> Self {
        x.into()
    }
}

pub mod ledger {
    use std::{borrow::Cow, fmt::Display};

    use crate::{
        context::{
            key::{self, KeyOwned},
            kv::{Item, PutOptions, Value},
            Context,
        },
        ledger::Transaction,
        prelude::*,
        util::Secret,
    };

    pub struct Put<V> {
        key: KeyOwned,
        current_value: V,
        previous_value: Option<Secret<Value>>,
    }

    impl<V> Put<V> {
        pub fn new(key: KeyOwned, current_value: V, previous_value: Option<Secret<Value>>) -> Self {
            Self {
                key,
                current_value,
                previous_value,
            }
        }
    }

    impl<V> Transaction for Put<V>
    where
        V: Into<Value> + Display + Send + 'static,
    {
        fn description(&self) -> Cow<'static, str> {
            "Put value".into()
        }

        fn detail(&self) -> Cow<'static, str> {
            let mut detail = "Key: ".to_owned();
            detail += self.key.as_str();
            detail += "\nCurrent Value: ";
            detail += &self.current_value.to_string();
            if let Some(previous_value) = &self.previous_value {
                detail += "\nPrevious Value: ";
                detail += &previous_value.to_string();
            }
            detail.into()
        }

        #[throws(anyhow::Error)]
        fn revert(mut self: Box<Self>) {
            let mut kv = Context::get_or_init().kv_mut();
            match self.previous_value.take() {
                Some(previous_value) => {
                    kv.put_value(
                        self.key,
                        previous_value,
                        PutOptions {
                            update: true,
                            ..Default::default()
                        },
                    )?;
                }
                None => {
                    kv.drop_item(&self.key, |_, _| ())?;
                }
            }
        }
    }

    pub struct Drop {
        template: KeyOwned,
        item: Item,
    }

    impl Drop {
        pub fn new(template: KeyOwned, item: Item) -> Self {
            Self { template, item }
        }
    }

    impl Transaction for Drop {
        fn description(&self) -> Cow<'static, str> {
            "Drop value".into()
        }

        fn detail(&self) -> Cow<'static, str> {
            let mut detail = "Key: ".to_owned();
            detail += self.template.as_str();
            detail += "\nItem:\n";
            detail += &self.item.to_string();
            detail.into()
        }

        #[throws(anyhow::Error)]
        fn revert(self: Box<Self>) {
            let known_prefix = key::get_known_prefix_for_template(&self.template);
            let mut kv = Context::get_or_init().kv_mut();
            for (key, value) in self.item.into_key_values() {
                kv.put_value(
                    known_prefix.join(&key),
                    value,
                    PutOptions {
                        update: false,
                        temporary: false,
                    },
                )?;
            }
        }
    }
}
