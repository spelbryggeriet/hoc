use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Seek, SeekFrom},
    path::PathBuf,
};

use hoc_log::error;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use self::{history::History, temp_path::TempPath};

pub mod history;
pub mod kv;
pub mod temp_path;

const ENV_HOME: &str = "HOME";

#[derive(Debug, Error)]
pub enum Error {
    #[error("environment variable {0}: {1}")]
    EnvVar(String, env::VarError),

    #[error("io: {0}")]
    Io(#[from] io::Error),

    #[error("serde yaml: {0}")]
    SerdeYaml(#[from] serde_yaml::Error),

    #[error("history: {0}")]
    History(#[from] history::Error),

    #[error("key/value registry: {0}")]
    KvStore(#[from] kv::Error),
}

impl From<Error> for hoc_log::Error {
    fn from(err: Error) -> Self {
        error!("{err}").unwrap_err()
    }
}

pub struct Context {
    state: State,
    file: File,
    _temp_dir: TempPath,
}

#[derive(Debug, Serialize, Deserialize)]
struct State {
    history: History,
    registry: kv::Store,
}

impl Context {
    pub const CONTEXT_STATE_FILE_NAME: &'static str = "context.yaml";
    pub const CONTEXT_DIR: &'static str = ".hoc";
    pub const WORK_DIR_PARENT: &'static str = "work";
    pub const TEMP_DIR_PARENT: &'static str = "temp";
    pub const REGISTRY_FILES_DIR: &'static str = "files";

    pub fn load() -> Result<Self, Error> {
        let context_dir_path = Self::get_context_dir();
        fs::create_dir_all(&context_dir_path)?;

        let registry_files_dir = Self::get_registry_files_dir();
        fs::create_dir_all(registry_files_dir)?;

        let temp_dir_path = Self::get_temp_dir();
        match temp_dir_path.metadata() {
            Ok(_) => fs::remove_dir_all(&temp_dir_path)?,
            Err(err) if err.kind() == io::ErrorKind::NotFound => (),
            Err(err) => return Err(err.into()),
        }
        fs::create_dir_all(&temp_dir_path)?;
        let _temp_dir = TempPath::from_path(temp_dir_path);

        let context_file_path = context_dir_path.join(Self::CONTEXT_STATE_FILE_NAME);
        let context = match OpenOptions::new()
            .read(true)
            .write(true)
            .open(&context_file_path)
        {
            Ok(file) => {
                let state: State = serde_yaml::from_reader(&file)?;
                Self {
                    state,
                    file,
                    _temp_dir,
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let file = File::create(&context_file_path)?;
                let context = Self {
                    state: State {
                        history: Default::default(),
                        registry: kv::Store::new(Self::get_registry_files_dir()),
                    },
                    file,
                    _temp_dir,
                };
                serde_yaml::to_writer(&context.file, &context.state)?;
                context
            }
            Err(error) => return Err(error.into()),
        };

        Ok(context)
    }

    pub fn get_temp_dir() -> PathBuf {
        let mut path = Self::get_context_dir();
        path.push(Self::TEMP_DIR_PARENT);
        path
    }

    fn get_context_dir() -> PathBuf {
        let home = env::var(ENV_HOME).unwrap();
        let mut context_path = PathBuf::new();
        context_path.push(home);
        context_path.push(Self::CONTEXT_DIR);
        context_path
    }

    fn get_registry_files_dir() -> PathBuf {
        let mut path = Self::get_context_dir();
        path.push(Self::REGISTRY_FILES_DIR);
        path
    }

    pub fn history(&self) -> &History {
        &self.state.history
    }

    pub fn history_mut(&mut self) -> &mut History {
        &mut self.state.history
    }

    pub fn registry(&self) -> &kv::Store {
        &self.state.registry
    }

    pub fn registry_mut(&mut self) -> &mut kv::Store {
        &mut self.state.registry
    }

    pub fn persist(&mut self) -> Result<(), Error> {
        self.state.registry.register_file_changes()?;
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        Ok(serde_yaml::to_writer(&self.file, &self.state)?)
    }
}
