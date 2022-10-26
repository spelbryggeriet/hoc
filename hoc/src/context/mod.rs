use std::{
    borrow::Cow,
    fmt::{self, Formatter},
    fs::{self, File},
    future::{Future, IntoFuture},
    io,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    pin::Pin,
};

use async_std::{fs::File as AsyncFile, path::PathBuf as AsyncPathBuf, sync::RwLock, task};
use once_cell::sync::OnceCell;
use serde::{de::Visitor, ser::SerializeMap, Deserialize, Deserializer, Serialize};
use strum::{Display, EnumIter};

use crate::prelude::*;
use cache::Cache;
use files::Files;
use key::Key;
use kv::{Kv, Value};

mod cache;
mod files;
pub mod key;
mod kv;

pub static CONTEXT: OnceCell<Context> = OnceCell::new();

pub struct Context {
    kv: RwLock<Kv>,
    files: RwLock<Files>,
    cache: RwLock<Cache>,
    data_dir: PathBuf,
}

impl Context {
    #[throws(anyhow::Error)]
    pub fn load<D, C>(data_dir: D, cache_dir: C) -> Self
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
        match File::options().read(true).write(true).open(&context_path) {
            Ok(file) => {
                trace!("Using pre-existing context file: {context_path:?}");

                debug!("Deserializing context from file");
                let mut context: Self = serde_yaml::from_reader(&file)?;
                task::block_on(context.files.write()).files_dir = data_dir.join("files");
                task::block_on(context.cache.write()).cache_dir = cache_dir;
                context.data_dir = data_dir;
                context
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                debug!("Creating new context file: {context_path:?}");

                debug!("Opening context file for creation");
                let file = File::options()
                    .write(true)
                    .create_new(true)
                    .open(&context_path)?;

                debug!("Creating context object");
                let context = Self {
                    kv: RwLock::new(Kv::new()),
                    files: RwLock::new(Files::new(data_dir.join("files"))),
                    cache: RwLock::new(Cache::new(cache_dir)),
                    data_dir,
                };

                debug!("Serializing context to file");
                serde_yaml::to_writer(file, &context)?;
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
    pub async fn files_create_file<'key, K>(&self, key: K) -> (AsyncFile, AsyncPathBuf)
    where
        K: Into<Cow<'key, Key>>,
    {
        self.files.write().await.create_file(key)?
    }

    #[throws(anyhow::Error)]
    pub async fn files_get_file<'key, K>(&self, key: K) -> (AsyncFile, AsyncPathBuf)
    where
        K: Into<Cow<'key, Key>>,
    {
        self.files.read().await.get_file(key)?
    }

    #[throws(anyhow::Error)]
    pub async fn cache_get_or_create_file_with<'key, K, F, E>(
        &self,
        key: K,
        f: F,
    ) -> (AsyncFile, AsyncPathBuf)
    where
        K: Into<Cow<'key, Key>>,
        F: for<'a> CachedFileFnOnce<'a, E>,
        E: Into<anyhow::Error> + 'static,
    {
        self.cache
            .write()
            .await
            .get_or_create_file_with(key, f)
            .await?
    }

    #[throws(anyhow::Error)]
    pub fn persist(&self) {
        info!("Persisting context");

        debug!("Opening context file for writing");
        let file = File::options()
            .write(true)
            .truncate(true)
            .open(&self.data_dir.join("context.yaml"))?;

        debug!("Serializing context to file");
        serde_yaml::to_writer(file, self)?;

        debug!("Context persisted");
    }
}

pub struct FileBuilder {
    key: Cow<'static, Key>,
}

impl FileBuilder {
    pub fn new(key: Cow<'static, Key>) -> Self {
        Self { key }
    }

    #[throws(anyhow::Error)]
    pub async fn get(self) -> (AsyncFile, AsyncPathBuf) {
        CONTEXT
            .get()
            .expect(EXPECT_CONTEXT_INITIALIZED)
            .files_get_file(self.key)
            .await?
    }

    #[throws(anyhow::Error)]
    pub async fn create(self) -> (AsyncFile, AsyncPathBuf) {
        CONTEXT
            .get()
            .expect(EXPECT_CONTEXT_INITIALIZED)
            .files_create_file(self.key)
            .await?
    }

    #[throws(anyhow::Error)]
    pub async fn cached<F, E>(self, f: F) -> (AsyncFile, AsyncPathBuf)
    where
        F: for<'a> CachedFileFnOnce<'a, E>,
        E: Into<anyhow::Error> + 'static,
    {
        CONTEXT
            .get()
            .expect(EXPECT_CONTEXT_INITIALIZED)
            .cache_get_or_create_file_with(self.key, f)
            .await?
    }
}

pub trait CachedFileFnOnce<'file, E>: FnOnce(&'file mut AsyncFile) -> Self::Fut {
    type Fut: Future<Output = Result<(), E>>;
}

impl<'file, F, Fut, E> CachedFileFnOnce<'file, E> for F
where
    F: FnOnce(&'file mut AsyncFile) -> Fut,
    Fut: Future<Output = Result<(), E>>,
{
    type Fut = Fut;
}

type FileBuilderFuture = Pin<Box<dyn Future<Output = anyhow::Result<(AsyncFile, AsyncPathBuf)>>>>;

impl IntoFuture for FileBuilder {
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
        task::block_on(async {
            let mut context = serializer.serialize_map(Some(3))?;
            context.serialize_entry("kv", &*self.kv.read().await)?;
            context.serialize_entry("files", &*self.files.read().await)?;
            context.serialize_entry("cache", &*self.cache.read().await)?;
            context.end()
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

#[derive(Display, EnumIter)]
enum Action {
    Abort,
    Skip,
    Overwrite,
}
