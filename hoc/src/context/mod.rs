use std::{borrow::Cow, fs, io, os::unix::fs::PermissionsExt, path::PathBuf, sync::Mutex};

use async_std::fs::File;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};

use crate::prelude::*;
use kv::{Kv, Value};

mod kv;

pub static CONTEXT: OnceCell<Context> = OnceCell::new();

#[derive(Serialize, Deserialize)]
pub struct Context {
    kv: Mutex<Kv>,

    #[serde(skip)]
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
        match fs::File::options()
            .read(true)
            .write(true)
            .open(&context_path)
        {
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
                let file = fs::File::options()
                    .write(true)
                    .create_new(true)
                    .open(&context_path)?;

                debug!("Creating context object");
                let context = Self {
                    kv: Mutex::new(Kv::new(data_dir.join("files"))),
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
        self.kv
            .lock()
            .expect(EXPECT_THREAD_NOT_POSIONED)
            .put_value(&*key, value)
            .await?;
    }

    #[throws(anyhow::Error)]
    pub async fn kv_create_file(&self, path: Cow<'static, str>) -> (File, PathBuf) {
        self.kv
            .lock()
            .expect(EXPECT_THREAD_NOT_POSIONED)
            .create_file(&*path)
            .await?
    }

    #[throws(anyhow::Error)]
    pub fn persist(&self) {
        info!("Persisting context");

        debug!("Opening context file for writing");
        let file = fs::File::options()
            .write(true)
            .truncate(true)
            .open(&self.data_dir.join("context.yaml"))?;

        debug!("Serializing context to file");
        serde_yaml::to_writer(file, self)?;

        debug!("Context persisted");
    }
}
