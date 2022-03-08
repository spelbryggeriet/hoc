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

use self::{
    step_history::{StepHistory, StepHistoryIndex, StepHistoryMap},
    temp_path::TempPath,
};
use crate::procedure::{Attribute, Procedure};

pub mod step_history;
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

    #[error("step history: {0}")]
    StepHistory(#[from] step_history::Error),

    #[error("step history already exist: {} with attributes {{{}}}",
        _0.name(),
        _0.attributes()
            .iter()
            .map(|Attribute { key, value }| format!("{key:?}: {value}"))
            .collect::<Vec<_>>()
            .join(", "))]
    StepHistoryAlreadyExist(StepHistoryIndex),
}

impl From<Error> for hoclog::Error {
    fn from(err: Error) -> Self {
        error!(err.to_string()).unwrap_err()
    }
}

#[derive(Debug, Serialize)]
pub struct Context {
    #[serde(flatten)]
    step_history_map: StepHistoryMap,

    #[serde(skip_serializing)]
    file: File,

    #[serde(skip_serializing)]
    _temp_dir: TempPath,
}

impl Index<&StepHistoryIndex> for Context {
    type Output = StepHistory;

    fn index(&self, index: &StepHistoryIndex) -> &Self::Output {
        &self.step_history_map.0[index]
    }
}

impl IndexMut<&StepHistoryIndex> for Context {
    fn index_mut(&mut self, index: &StepHistoryIndex) -> &mut Self::Output {
        self.step_history_map.0.get_mut(index).unwrap()
    }
}

impl Context {
    pub const CONTEXT_FILE_NAME: &'static str = "context.yaml";
    pub const CONTEXT_DIR: &'static str = ".hoc";
    pub const WORK_DIR_PARENT: &'static str = "work";
    pub const TEMP_DIR_PARENT: &'static str = "temp";

    pub fn load() -> Result<Self, Error> {
        let context_dir_path = Self::get_context_dir();
        fs::create_dir_all(&context_dir_path)?;

        let work_dir_parent_path = Self::get_work_dir_parent();
        fs::create_dir_all(work_dir_parent_path)?;

        let temp_dir_path = Self::get_temp_dir();
        match temp_dir_path.metadata() {
            Ok(_) => fs::remove_dir_all(&temp_dir_path)?,
            Err(err) if err.kind() == io::ErrorKind::NotFound => (),
            Err(err) => return Err(err.into()),
        }
        fs::create_dir_all(&temp_dir_path)?;
        let _temp_dir = TempPath::from_path(temp_dir_path);

        let context_file_path = context_dir_path.join(Self::CONTEXT_FILE_NAME);
        let context = match OpenOptions::new()
            .read(true)
            .write(true)
            .open(&context_file_path)
        {
            Ok(file) => {
                let step_history_map: StepHistoryMap = serde_yaml::from_reader(&file)?;
                Self {
                    step_history_map,
                    file,
                    _temp_dir,
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let file = File::create(&context_file_path)?;
                let context = Self {
                    step_history_map: Default::default(),
                    file,
                    _temp_dir,
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

    pub fn get_temp_dir() -> PathBuf {
        let mut path = Self::get_context_dir();
        path.push(Self::TEMP_DIR_PARENT);
        path
    }

    pub fn get_work_dir<P: Procedure>(attributes: &[Attribute]) -> PathBuf {
        let mut path = Self::get_work_dir_parent();

        let mut hasher = DefaultHasher::new();
        P::NAME.hash(&mut hasher);
        attributes.hash(&mut hasher);
        let hash = hasher.finish();

        path.push(format!("{}_{hash:016x}", P::NAME));
        path
    }

    pub fn get_step_history_index<P: Procedure>(&self, procedure: &P) -> Option<StepHistoryIndex> {
        let cache_index = StepHistoryIndex(P::NAME.to_string(), procedure.get_attributes());
        self.step_history_map
            .0
            .contains_key(&cache_index)
            .then(|| cache_index)
    }

    pub fn add_step_history<P: Procedure>(
        &mut self,
        procedure: &P,
    ) -> Result<StepHistoryIndex, Error> {
        fs::create_dir(Self::get_work_dir::<P>(&procedure.get_attributes()))?;

        let cache = StepHistory::new::<P>(procedure)?;
        let cache_index = StepHistoryIndex(P::NAME.to_string(), procedure.get_attributes());

        if self.step_history_map.0.contains_key(&cache_index) {
            return Err(Error::StepHistoryAlreadyExist(cache_index));
        }

        self.step_history_map.0.insert(cache_index.clone(), cache);
        Ok(cache_index)
    }

    pub fn persist(&mut self) -> Result<(), Error> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        Ok(serde_yaml::to_writer(&self.file, self)?)
    }
}
