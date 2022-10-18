use std::{
    borrow::Cow,
    fs::{self, File},
    io,
    path::PathBuf,
    sync::Mutex,
};

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
    file_path: PathBuf,
}

impl Context {
    #[throws(anyhow::Error)]
    pub fn load<P: Into<PathBuf>>(context_path: P) -> Self {
        debug!("Loading context");

        let context_path = context_path.into();

        debug!("Retrieving parent directory to context path");
        let context_dir = context_path
            .parent()
            .context("context path does not have any parent directories")?;

        debug!("Creating context directories");
        fs::create_dir_all(context_dir)?;

        debug!("Opening context file");
        match File::options().read(true).write(true).open(&context_path) {
            Ok(file) => {
                trace!("Using pre-existing context file: {context_path:?}");

                debug!("Deserializing context from file");
                let mut context: Self = serde_yaml::from_reader(&file)?;
                context.file_path = context_path;
                context
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                info!("Creating new context file: {context_path:?}");

                debug!("Opening context file for creation");
                let file = File::create(&context_path)?;

                debug!("Creating context object");
                let context = Self {
                    kv: Mutex::new(Kv::new()),
                    file_path: context_path,
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
    pub fn persist(&self) {
        info!("Persisting context");

        debug!("Opening context file for writing");
        let file = File::options()
            .write(true)
            .truncate(true)
            .open(&self.file_path)?;

        debug!("Serializing context to file");
        serde_yaml::to_writer(file, self)?;
    }
}
