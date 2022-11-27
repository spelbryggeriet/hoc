use std::{
    borrow::Cow, fmt::Display, fs::File as BlockingFile, future::Future, future::IntoFuture, io,
    marker::PhantomData, os::unix::fs::PermissionsExt, pin::Pin,
};

use once_cell::sync::OnceCell;
use serde::{ser::SerializeMap, Deserialize, Deserializer, Serialize};
use thiserror::Error;
use tokio::{
    fs::{File, OpenOptions},
    runtime::Handle,
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard},
    task,
};

use self::{
    fs::{cache::Cache, files::Files, temp::Temp, CachedFileFn},
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

fn default_temp_rw_lock() -> RwLock<Temp> {
    RwLock::new(Temp::new())
}

#[derive(Deserialize)]
pub struct Context {
    #[serde(deserialize_with = "deserialize_rw_lock")]
    kv: RwLock<Kv>,
    #[serde(deserialize_with = "deserialize_rw_lock")]
    files: RwLock<Files>,
    #[serde(skip, default = "default_temp_rw_lock")]
    temp: RwLock<Temp>,
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
            temp: RwLock::new(Temp::new()),
            cache: RwLock::new(Cache::new()),
        }
    }

    #[throws(anyhow::Error)]
    pub async fn load(&self) {
        debug!("Loading context");

        let data_dir = crate::data_dir();
        let cache_dir = crate::cache_dir();

        debug!("Creating data directory");
        tokio::fs::create_dir_all(&data_dir).await?;

        debug!("Creating cache directory");
        tokio::fs::create_dir_all(&cache_dir).await?;

        debug!("Setting data directory permissions");
        let mut data_permissions = data_dir.metadata()?.permissions();
        data_permissions.set_mode(0o700);
        tokio::fs::set_permissions(&data_dir, data_permissions).await?;

        debug!("Setting cache directory permissions");
        let mut cache_permissions = cache_dir.metadata()?.permissions();
        cache_permissions.set_mode(0o700);
        tokio::fs::set_permissions(&cache_dir, cache_permissions).await?;

        let context_path = data_dir.join(Self::CONTEXT_FILENAME);

        debug!("Opening context file");
        match OpenOptions::new()
            .read(true)
            .write(true)
            .open(&context_path)
            .await
        {
            Ok(file) => {
                trace!("Using pre-existing context file: {context_path:?}");

                debug!("Deserializing context from file");
                let context: Self = serde_yaml::from_reader(File::into_std(file).await)?;
                *self.kv.write().await = context.kv.into_inner();
                *self.files.write().await = context.files.into_inner();
                *self.temp.write().await = context.temp.into_inner();
                *self.cache.write().await = context.cache.into_inner();
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                debug!("No context file found");
            }
            Err(error) => throw!(error),
        }
    }

    pub async fn kv(&self) -> RwLockReadGuard<Kv> {
        self.kv.read().await
    }

    pub async fn kv_mut(&self) -> RwLockWriteGuard<Kv> {
        self.kv.write().await
    }

    pub async fn files(&self) -> RwLockReadGuard<Files> {
        self.files.read().await
    }

    pub async fn files_mut(&self) -> RwLockWriteGuard<Files> {
        self.files.write().await
    }

    pub async fn temp_mut(&self) -> RwLockWriteGuard<Temp> {
        self.temp.write().await
    }

    pub async fn cache_mut(&self) -> RwLockWriteGuard<Cache> {
        self.cache.write().await
    }

    #[throws(anyhow::Error)]
    pub async fn persist(&self) {
        progress!("Persisting context");

        debug!("Dropping temporary values");
        self.kv.write().await.drop_temporary_values();

        debug!("Opening context file for writing");
        let file = BlockingFile::options()
            .write(true)
            .truncate(true)
            .create(true)
            .open(crate::data_dir().join(Self::CONTEXT_FILENAME))?;

        debug!("Serializing context to file");
        serde_yaml::to_writer(file, self)?;

        debug!("Cleaning temporary files");
        let temp = self.temp.write();
        temp.await.clean().await?;
    }
}

impl Serialize for Context {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        task::block_in_place(|| {
            Handle::current().block_on(async {
                let mut context = serializer.serialize_map(Some(3))?;
                context.serialize_entry("kv", &*self.kv.read().await)?;
                context.serialize_entry("files", &*self.files.read().await)?;
                context.serialize_entry("cache", &*self.cache.read().await)?;
                context.end()
            })
        })
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
    pub async fn get(self) -> Item {
        Context::get_or_init().kv().await.get_item(&self.key)?
    }
}

impl<'a, O> KvBuilder<'a, O> {
    #[throws(kv::Error)]
    pub async fn put<V>(self, value: V)
    where
        V: Into<Value> + Clone + Display + Send + 'static,
    {
        let previous_value = Context::get_or_init().kv_mut().await.put_value(
            &self.key,
            value.clone(),
            PutOptions {
                force: false,
                temporary: self.temporary,
            },
        )?;

        if !self.temporary && previous_value != Some(None) {
            Ledger::get_or_init().lock().await.add(ledger::Put::new(
                self.key.into_owned(),
                value,
                previous_value.flatten().map(Secret::new),
            ));
        }
    }
}

type KvBuilderFuture<'a> = Pin<Box<dyn Future<Output = Result<Item, kv::Error>> + Send + 'a>>;

impl<'a> IntoFuture for KvBuilder<'a, All> {
    type IntoFuture = KvBuilderFuture<'a>;
    type Output = <KvBuilderFuture<'a> as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.get())
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
    _Custom(#[from] anyhow::Error),
}

pub mod ledger {
    use std::{borrow::Cow, fmt::Display};

    use async_trait::async_trait;

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

    #[async_trait]
    impl<V: Into<Value> + Display + Send + 'static> Transaction for Put<V> {
        fn description(&self) -> Cow<'static, str> {
            "Put value".into()
        }

        fn detail(&self) -> Cow<'static, str> {
            format!("Value to revert: {}", self.current_value).into()
        }

        async fn revert(mut self: Box<Self>) -> anyhow::Result<()> {
            let mut kv = Context::get_or_init().kv_mut().await;
            match self.previous_value.take() {
                Some(previous_value) => {
                    kv.put_value(
                        self.key,
                        previous_value,
                        PutOptions {
                            force: true,
                            ..Default::default()
                        },
                    )?;
                }
                None => {
                    kv.drop_value(&self.key, true)?;
                }
            }
            Ok(())
        }
    }
}
