use std::{
    borrow::Cow,
    fmt::{self, Formatter},
    fs::{self, File},
    io,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
};

use async_std::{fs::File as AsyncFile, path::PathBuf as AsyncPathBuf, sync::RwLock, task};
use once_cell::sync::OnceCell;
use serde::{de::Visitor, ser::SerializeMap, Deserialize, Deserializer, Serialize};

use crate::prelude::*;
use kv::{Kv, Value};

mod kv;

pub static CONTEXT: OnceCell<Context> = OnceCell::new();

pub struct Context {
    kv: RwLock<Kv>,
    data_dir: PathBuf,
}

impl Context {
    #[throws(anyhow::Error)]
    pub fn load<P: Into<PathBuf>>(data_dir: P) -> Self {
        debug!("Loading context");

        let data_dir = data_dir.into();

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
                    kv: RwLock::new(Kv::new(data_dir.join("files"))),
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
    pub async fn kv_put_value(&self, key: Cow<'static, str>, value: impl Into<Value>) {
        self.kv.write().await.put_value(&*key, value)?;
    }

    #[throws(anyhow::Error)]
    pub async fn kv_create_file(&self, path: Cow<'static, str>) -> (AsyncFile, AsyncPathBuf) {
        self.kv.write().await.create_file(&*path)?
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

impl Serialize for Context {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        task::block_on(async {
            let mut context = serializer.serialize_map(Some(1))?;
            context.serialize_entry("kv", &*self.kv.read().await)?;
            context.end()
        })
    }
}

impl<'de> Deserialize<'de> for Context {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct KvField;

        struct FieldVisitor;
        impl<'de> Visitor<'de> for FieldVisitor {
            type Value = KvField;

            fn expecting(&self, f: &mut Formatter) -> fmt::Result {
                write!(f, "a field identifier")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "kv" => Ok(KvField),
                    key => return Err(serde::de::Error::custom(format!("unexpected key: {key}"))),
                }
            }
        }

        impl<'de> Deserialize<'de> for KvField {
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

            fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
                formatter.write_str("a map")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut kv = None;
                while let Some(_) = map.next_key::<KvField>()? {
                    kv.replace(map.next_value()?);
                }

                let kv: Kv = kv.ok_or(serde::de::Error::custom("missing key: kv"))?;

                Ok(Context {
                    kv: RwLock::new(kv),
                    data_dir: PathBuf::new(),
                })
            }
        }

        deserializer.deserialize_map(ContextVisitor)
    }
}
