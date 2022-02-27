use std::{
    collections::hash_map::DefaultHasher,
    env,
    fs::{self, File, OpenOptions},
    hash::{Hash, Hasher},
    io::{self, Seek, SeekFrom},
    ops::{Index, IndexMut},
    path::PathBuf,
};

use hoclog::error;
use serde::Serialize;
use thiserror::Error;

use self::steps::{StepsIndex, StepsMap};
use crate::{Procedure, Steps};

pub mod steps;

const ENV_HOME: &str = "HOME";

#[derive(Debug, Error)]
pub enum Error {
    #[error("environment variable {0}: {1}")]
    EnvVar(String, env::VarError),

    #[error("io: {0}")]
    Io(#[from] io::Error),

    #[error("serde yaml: {0}")]
    SerdeYaml(#[from] serde_yaml::Error),

    #[error("steps: {0}")]
    Steps(#[from] steps::Error),

    #[error("steps already exist: {} with attributes {{{}}}",
        _0.name(),
        _0.attributes()
            .iter()
            .map(|(k, v)| format!("{k:?}: {v}"))
            .collect::<Vec<_>>()
            .join(", "))]
    StepsAlreadyExist(StepsIndex),
}

impl From<Error> for hoclog::Error {
    fn from(err: Error) -> Self {
        error!(err.to_string()).unwrap_err()
    }
}

#[derive(Debug, Serialize)]
pub struct Context {
    #[serde(flatten)]
    steps: StepsMap,

    #[serde(skip_serializing)]
    file: File,
}

impl Index<&StepsIndex> for Context {
    type Output = Steps;

    fn index(&self, index: &StepsIndex) -> &Self::Output {
        &self.steps.0[index]
    }
}

impl IndexMut<&StepsIndex> for Context {
    fn index_mut(&mut self, index: &StepsIndex) -> &mut Self::Output {
        self.steps.0.get_mut(index).unwrap()
    }
}

impl Context {
    pub const CONTEXT_FILE_NAME: &'static str = "context.yaml";
    pub const CONTEXT_DIR: &'static str = ".hoc";
    pub const WORK_DIR_PARENT: &'static str = "workdir";

    pub fn load() -> Result<Self, Error> {
        let mut work_dir_parent_path = Self::get_work_dir_parent();

        match fs::metadata(&work_dir_parent_path) {
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir_all(&work_dir_parent_path)?;
            }
            Err(error) => return Err(error.into()),
        }

        work_dir_parent_path.pop();
        work_dir_parent_path.push(Self::CONTEXT_FILE_NAME);
        let context_dir_path = work_dir_parent_path;

        let context = match OpenOptions::new()
            .read(true)
            .write(true)
            .open(&context_dir_path)
        {
            Ok(file) => {
                let caches: StepsMap = serde_yaml::from_reader(&file)?;
                Self {
                    steps: caches,
                    file,
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let file = File::create(&context_dir_path)?;
                let context = Self {
                    steps: Default::default(),
                    file,
                };
                serde_yaml::to_writer(&context.file, &context)?;
                context
            }
            Err(error) => return Err(error.into()),
        };

        Ok(context)
    }

    fn get_context_dir() -> PathBuf {
        let home = env::var(ENV_HOME).unwrap();
        let mut context_path = PathBuf::new();
        context_path.push(home);
        context_path.push(Self::CONTEXT_DIR);
        context_path
    }

    fn get_work_dir_parent() -> PathBuf {
        let mut path = Self::get_context_dir();
        path.push(Self::WORK_DIR_PARENT);
        path
    }

    pub fn get_work_dir<P: Procedure>(procedure: &P) -> PathBuf {
        let mut path = Self::get_work_dir_parent();

        let mut hasher = DefaultHasher::new();
        P::NAME.hash(&mut hasher);
        procedure.get_attributes().hash(&mut hasher);
        let hash = hasher.finish();

        path.push(format!("{}_{hash:016x}", P::NAME));
        path
    }

    pub fn get_steps_index<P: Procedure>(&self, procedure: &P) -> Option<StepsIndex> {
        let cache_index = StepsIndex(P::NAME.to_string(), procedure.get_attributes());
        self.steps.0.contains_key(&cache_index).then(|| cache_index)
    }

    pub fn add_steps<P: Procedure>(&mut self, procedure: &P) -> Result<StepsIndex, Error> {
        fs::create_dir(Self::get_work_dir(procedure))?;

        let cache = Steps::new::<P>(procedure)?;
        let cache_index = StepsIndex(P::NAME.to_string(), procedure.get_attributes());

        if self.steps.0.contains_key(&cache_index) {
            return Err(Error::StepsAlreadyExist(cache_index));
        }

        self.steps.0.insert(cache_index.clone(), cache);
        Ok(cache_index)
    }

    pub fn persist(&mut self) -> Result<(), Error> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        Ok(serde_yaml::to_writer(&self.file, self)?)
    }
}
