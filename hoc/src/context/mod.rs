use std::{
    borrow::Cow,
    fmt::{self, Formatter},
    fs::{self, File as BlockingFile},
    future::{Future, IntoFuture},
    io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    pin::Pin,
};

use once_cell::sync::OnceCell;
use serde::{de::Visitor, ser::SerializeMap, Deserialize, Deserializer, Serialize};
use tokio::{
    fs::{File, OpenOptions},
    runtime::Handle,
    sync::RwLock,
    task,
};

use crate::prelude::*;
use cache::Cache;
use files::Files;
use key::Key;
use kv::{Kv, Value};

mod cache;
mod files;
pub mod key;
mod kv;

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
        fs::create_dir_all(&data_dir)?;

        debug!("Setting data directory permissions");
        let mut permissions = data_dir.metadata()?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&data_dir, permissions)?;

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
                context.cache.write().await.cache_dir = cache_dir;
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
                    cache: RwLock::new(Cache::new(cache_dir)),
                    data_dir,
                };

                debug!("Serializing context to file");
                serde_yaml::to_writer(File::into_std(file).await, &context)?;
                context
            }
            Err(error) => throw!(error),
        }
    }

    #[throws(anyhow::Error)]
    pub async fn kv_put_value<'key, K, V>(&self, key: K, value: V)
    where
        K: Into<Cow<'key, Key>>,
        V: Into<Value>,
    {
        self.kv.write().await.put_value(key, value)?;
    }

    #[throws(anyhow::Error)]
    pub async fn files_create_file<'key, K>(&self, key: K) -> (File, PathBuf)
    where
        K: Into<Cow<'key, Key>>,
    {
        self.files.write().await.create_file(key)?
    }

    #[throws(anyhow::Error)]
    pub async fn files_get_file<'key, K>(&self, key: K) -> (File, PathBuf)
    where
        K: Into<Cow<'key, Key>>,
    {
        self.files.read().await.get_file(key)?
    }

    #[throws(anyhow::Error)]
    pub async fn cache_get_or_create_file_with<K, F>(&self, key: K, f: F) -> (File, PathBuf)
    where
        K: Into<Cow<'static, Key>>,
        F: for<'a> CachedFileFnOnce<'a>,
    {
        self.cache
            .write()
            .await
            .get_or_create_file_with(key, f)
            .await?
    }

    #[throws(anyhow::Error)]
    pub async fn cache_create_or_overwrite_file_with<K, F>(&self, key: K, f: F) -> (File, PathBuf)
    where
        K: Into<Cow<'static, Key>>,
        F: for<'a> CachedFileFnOnce<'a>,
    {
        self.cache
            .write()
            .await
            .create_or_overwrite_file_with(key, f)
            .await?
    }

    #[throws(anyhow::Error)]
    pub fn persist(&self) {
        info!("Persisting context");

        debug!("Opening context file for writing");
        let file = BlockingFile::options()
            .write(true)
            .truncate(true)
            .open(&self.data_dir.join("context.yaml"))?;

        debug!("Serializing context to file");
        serde_yaml::to_writer(file, self)?;

        debug!("Context persisted");
    }
}

pub struct FileBuilder<S> {
    key: Cow<'static, Key>,
    state: S,
}

pub struct Persisted(());
pub struct Cached<F> {
    file_cacher: F,
    clear: bool,
}

impl FileBuilder<Persisted> {
    pub fn new(key: Cow<'static, Key>) -> Self {
        Self {
            key,
            state: Persisted(()),
        }
    }

    #[throws(anyhow::Error)]
    pub async fn get(self) -> (File, PathBuf) {
        get_context().files_get_file(self.key).await?
    }

    #[throws(anyhow::Error)]
    pub async fn create(self) -> (File, PathBuf) {
        get_context().files_create_file(self.key).await?
    }

    pub fn cached<F>(self, file_cacher: F) -> FileBuilder<Cached<F>>
    where
        F: for<'a> CachedFileFnOnce<'a>,
    {
        FileBuilder {
            key: self.key,
            state: Cached {
                file_cacher,
                clear: false,
            },
        }
    }
}

impl<F> FileBuilder<Cached<F>>
where
    F: for<'a> CachedFileFnOnce<'a>,
{
    pub fn _clear_if_present(mut self) -> Self {
        self.state.clear = true;
        self
    }

    #[throws(anyhow::Error)]
    pub async fn get(self) -> (File, PathBuf) {
        if !self.state.clear {
            get_context()
                .cache_get_or_create_file_with(self.key, self.state.file_cacher)
                .await?
        } else {
            get_context()
                .cache_create_or_overwrite_file_with(self.key, self.state.file_cacher)
                .await?
        }
    }
}

pub trait CachedFileFnOnce<'a>: FnOnce(&'a mut File, &'a Path, bool) -> Self::Fut {
    type Fut: Future<Output = Result<(), Self::Error>>;
    type Error: Into<anyhow::Error> + 'static;
}

impl<'a, F, Fut, E> CachedFileFnOnce<'a> for F
where
    F: FnOnce(&'a mut File, &'a Path, bool) -> Fut,
    Fut: Future<Output = Result<(), E>>,
    E: Into<anyhow::Error> + 'static,
{
    type Fut = Fut;
    type Error = E;
}

type FileBuilderFuture = Pin<Box<dyn Future<Output = anyhow::Result<(File, PathBuf)>>>>;

impl IntoFuture for FileBuilder<Persisted> {
    type IntoFuture = FileBuilderFuture;
    type Output = <FileBuilderFuture as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.get())
    }
}

impl<F> IntoFuture for FileBuilder<Cached<F>>
where
    F: for<'a> CachedFileFnOnce<'a> + 'static,
{
    type IntoFuture = FileBuilderFuture;
    type Output = <FileBuilderFuture as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.get())
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
                    key => return Err(serde::de::Error::custom(format!("unexpected key: {key}"))),
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

                let kv: Kv = kv.ok_or(serde::de::Error::custom("missing key: kv"))?;
                let files: Files = files.ok_or(serde::de::Error::custom("missing key: files"))?;
                let cache: Cache = cache.ok_or(serde::de::Error::custom("missing key: cache"))?;

                Context {
                    kv: RwLock::new(kv),
                    files: RwLock::new(files),
                    cache: RwLock::new(cache),
                    data_dir: PathBuf::new(),
                }
            }
        }

        deserializer.deserialize_map(ContextVisitor)
    }
}
