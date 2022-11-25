use std::{
    borrow::Cow,
    fmt::{self, Display, Formatter},
    fs::File as BlockingFile,
    future::Future,
    future::IntoFuture,
    io,
    marker::PhantomData,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    pin::Pin,
};

use once_cell::sync::OnceCell;
use serde::{de::Visitor, ser::SerializeMap, Deserialize, Deserializer, Serialize};
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

static CONTEXT: OnceCell<Context> = OnceCell::new();

#[throws(anyhow::Error)]
pub async fn init<D, C>(data_dir: D, cache_dir: C)
where
    D: Into<PathBuf>,
    C: Into<PathBuf>,
{
    if CONTEXT.get().is_some() {
        panic!("context already initialized");
    }

    let context = Context::load(data_dir, cache_dir).await?;
    let _ = CONTEXT.set(context);
}

pub fn get_context() -> &'static Context {
    CONTEXT.get().expect("context is not initialized")
}

pub struct Context {
    kv: RwLock<Kv>,
    files: RwLock<Files>,
    temp: RwLock<Temp>,
    cache: RwLock<Cache>,
    data_dir: PathBuf,
}

impl Context {
    #[throws(anyhow::Error)]
    pub async fn load<D, C>(data_dir: D, cache_dir: C) -> Self
    where
        D: Into<PathBuf>,
        C: Into<PathBuf>,
    {
        debug!("Loading context");

        let data_dir = data_dir.into();
        let cache_dir = cache_dir.into();

        debug!("Creating data directory");
        tokio::fs::create_dir_all(&data_dir).await?;

        debug!("Setting data directory permissions");
        let mut permissions = data_dir.metadata()?.permissions();
        permissions.set_mode(0o700);
        tokio::fs::set_permissions(&data_dir, permissions).await?;

        let context_path = data_dir.join("context.yaml");

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
                let mut context: Self = serde_yaml::from_reader(File::into_std(file).await)?;
                context.files.write().await.files_dir = data_dir.join("files");
                context.temp.write().await.files_dir = cache_dir.join("temp");
                context.cache.write().await.cache_dir = cache_dir.join("cache");
                context.data_dir = data_dir;
                context
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                debug!("Creating new context file: {context_path:?}");

                debug!("Opening context file for creation");
                let file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&context_path)
                    .await?;

                debug!("Creating context object");
                let context = Self {
                    kv: RwLock::new(Kv::new()),
                    files: RwLock::new(Files::new(data_dir.join("files"))),
                    temp: RwLock::new(Temp::new(cache_dir.join("temp"))),
                    cache: RwLock::new(Cache::new(cache_dir.join("cache"))),
                    data_dir,
                };

                debug!("Serializing context to file");
                serde_yaml::to_writer(File::into_std(file).await, &context)?;
                context
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
            .open(&self.data_dir.join("context.yaml"))?;

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

impl<'de> Deserialize<'de> for Context {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        enum Field {
            Kv,
            Files,
            Cache,
        }

        struct FieldVisitor;
        impl<'de> Visitor<'de> for FieldVisitor {
            type Value = Field;
            fn expecting(&self, f: &mut Formatter) -> fmt::Result {
                write!(f, "a field identifier")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "kv" => Ok(Field::Kv),
                    "files" => Ok(Field::Files),
                    "cache" => Ok(Field::Cache),
                    key => Err(serde::de::Error::custom(format!("unexpected key: {key}"))),
                }
            }
        }

        impl<'de> Deserialize<'de> for Field {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_identifier(FieldVisitor)
            }
        }

        struct ContextVisitor;
        impl<'de> Visitor<'de> for ContextVisitor {
            type Value = Context;

            #[throws(fmt::Error)]
            fn expecting(&self, formatter: &mut Formatter) {
                formatter.write_str("a map")?;
            }

            #[throws(A::Error)]
            fn visit_map<A>(self, mut map: A) -> Self::Value
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut kv = None;
                let mut files = None;
                let mut cache = None;
                while let Some(field) = map.next_key::<Field>()? {
                    match field {
                        Field::Kv => kv = map.next_value()?,
                        Field::Files => files = map.next_value()?,
                        Field::Cache => cache = map.next_value()?,
                    }
                }

                let kv: Kv = kv.ok_or_else(|| serde::de::Error::custom("missing key: kv"))?;
                let files: Files =
                    files.ok_or_else(|| serde::de::Error::custom("missing key: files"))?;
                let cache: Cache =
                    cache.ok_or_else(|| serde::de::Error::custom("missing key: cache"))?;

                Context {
                    kv: RwLock::new(kv),
                    files: RwLock::new(files),
                    temp: RwLock::new(Temp::empty()),
                    cache: RwLock::new(cache),
                    data_dir: PathBuf::new(),
                }
            }
        }

        deserializer.deserialize_map(ContextVisitor)
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
        get_context().kv().await.get_item(&self.key)?
    }
}

impl<'a, O> KvBuilder<'a, O> {
    #[throws(kv::Error)]
    pub async fn put<V>(self, value: V)
    where
        V: Into<Value> + Clone + Display + Send + 'static,
    {
        let previous_value = get_context().kv_mut().await.put_value(
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
            self,
            key::KeyOwned,
            kv::{PutOptions, Value},
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
            let mut kv = context::get_context().kv_mut().await;
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
