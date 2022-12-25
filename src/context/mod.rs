use std::{
    borrow::Cow,
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
    fs::{cache::Cache, files::Files},
    key::Key,
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
}

impl Context {
    const CONTEXT_FILENAME: &str = "context.yaml";

    pub fn get_or_init() -> &'static Context {
        static CONTEXT: OnceCell<Context> = OnceCell::new();

        CONTEXT.get_or_init(Context::new)
    }

    fn new() -> Self {
        Self {
            kv: RwLock::new(Kv::new()),
            files: RwLock::new(Files::new()),
            cache: RwLock::new(Cache::new()),
        }
    }

    #[throws(anyhow::Error)]
    pub fn load(&self) {
        debug!("Loading context");

        let data_dir = crate::local_data_dir();
        let cache_dir = crate::local_cache_dir();

        debug!("Creating data directory");
        std::fs::create_dir_all(&data_dir)?;

        debug!("Creating cache directory");
        std::fs::create_dir_all(&cache_dir)?;

        debug!("Setting data directory permissions");
        let mut data_permissions = data_dir.metadata()?.permissions();
        data_permissions.set_mode(0o700);
        std::fs::set_permissions(&data_dir, data_permissions)?;

        debug!("Setting cache directory permissions");
        let mut cache_permissions = cache_dir.metadata()?.permissions();
        cache_permissions.set_mode(0o700);
        std::fs::set_permissions(&cache_dir, cache_permissions)?;

        let context_path = data_dir.join(Self::CONTEXT_FILENAME);

        debug!("Opening context file");
        match File::options().read(true).write(true).open(&context_path) {
            Ok(file) => {
                trace!("Using pre-existing context file: {context_path:?}");

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
            .open(crate::local_data_dir().join(Self::CONTEXT_FILENAME))?;

        debug!("Serializing context to file");
        serde_yaml::to_writer(file, self)?;
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

    #[throws(kv::Error)]
    pub fn get(self) -> Item {
        Context::get_or_init().kv().get_item(&self.key)?
    }

    #[throws(kv::Error)]
    pub fn update<V>(self, value: V)
    where
        V: Into<Value> + Clone + Display + Send + 'static,
    {
        self.put_or_update(value, true)?;
    }
}

impl<'a, O> KvBuilder<'a, O> {
    #[throws(kv::Error)]
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

    #[throws(kv::Error)]
    pub fn put<V>(self, value: V)
    where
        V: Into<Value> + Clone + Display + Send + 'static,
    {
        self.put_or_update(value, false)?;
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
    Custom(#[from] anyhow::Error),
}

pub mod ledger {
    use std::{borrow::Cow, fmt::Display};

    use crate::{
        context::{
            key::KeyOwned,
            kv::{PutOptions, Value},
            Context,
        },
        ledger::Transaction,
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

    impl<V: Into<Value> + Display + Send + 'static> Transaction for Put<V> {
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

        fn revert(mut self: Box<Self>) -> anyhow::Result<()> {
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
                    kv.drop_value(&self.key)?;
                }
            }
            Ok(())
        }
    }
}
