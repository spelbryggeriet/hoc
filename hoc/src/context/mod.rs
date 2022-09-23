use std::{
    fs::{self, File},
    io,
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

use self::kv::Kv;
use crate::prelude::*;

mod kv;

#[derive(Serialize, Deserialize)]
pub struct Context {
    pub kv: Kv,

    #[serde(skip)]
    file_path: PathBuf,
}

impl Context {
    #[throws(anyhow::Error)]
    pub fn load<P: Into<PathBuf>>(context_path: P) -> Self {
        let context_path = context_path.into();

        debug!("retrieving parent directory to context path");
        let context_dir = context_path
            .parent()
            .context("context path does not have any parent directories")?;

        debug!("creating context directories");
        fs::create_dir_all(context_dir)?;

        debug!("opening context file");
        match File::options().read(true).write(true).open(&context_path) {
            Ok(file) => {
                info!("using pre-existing context file: {context_path:?}");

                debug!("deserializing context from file");
                let mut context: Self = serde_yaml::from_reader(&file)?;
                context.file_path = context_path;
                context
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                info!("creating new context file: {context_path:?}");

                debug!("opening context file for creation");
                let file = File::create(&context_path)?;

                debug!("creating context object");
                let context = Self {
                    kv: Kv::new(),
                    file_path: context_path,
                };

                debug!("serializing context to file");
                serde_yaml::to_writer(file, &context)?;
                context
            }
            Err(error) => throw!(error),
        }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        info!("persisting context");

        debug!("opening context file for writing");
        let file = match File::options()
            .write(true)
            .truncate(true)
            .open(&self.file_path)
        {
            Ok(file) => file,
            Err(err) => {
                error!("failed to open file for writing: {err}");
                return;
            }
        };

        debug!("serializing context to file");
        if let Err(err) = serde_yaml::to_writer(file, self) {
            error!("failed to persist context: {err}");
            return;
        }
    }
}
