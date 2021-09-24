use std::{
    collections::HashMap,
    env,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, Seek, SeekFrom},
    ops::{Index, IndexMut},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    procedure::{ProcedureState, ProcedureStateId},
    Error, Result,
};

use self::dir_state::{DirectoryState, FileWriter};

pub mod dir_state;

const ENV_HOME: &str = "HOME";

#[derive(Debug, Serialize)]
pub struct Context {
    proc_caches: HashMap<String, ProcedureCache>,

    #[serde(skip_serializing)]
    file: File,
}

impl Index<&str> for Context {
    type Output = ProcedureCache;

    fn index(&self, index: &str) -> &Self::Output {
        &self.proc_caches[index]
    }
}

impl IndexMut<&str> for Context {
    fn index_mut(&mut self, index: &str) -> &mut Self::Output {
        self.proc_caches.get_mut(index).unwrap()
    }
}

impl Context {
    pub const CONTEXT_FILE_NAME: &'static str = "context.yaml";
    pub const CONTEXT_DIR: &'static str = ".hoc";
    pub const WORK_DIR: &'static str = "workdir";
    pub const FILES_DIR: &'static str = "files";

    pub fn load() -> Result<Self> {
        let mut work_dir_path = Self::get_work_dir()?;

        match fs::metadata(&work_dir_path) {
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir_all(&work_dir_path)?;
            }
            Err(error) => return Err(error.into()),
        }

        work_dir_path.pop();
        work_dir_path.push(Self::CONTEXT_FILE_NAME);
        let context_dir_path = work_dir_path;

        match OpenOptions::new()
            .read(true)
            .write(true)
            .open(&context_dir_path)
        {
            Ok(file) => {
                let mut map: HashMap<String, HashMap<String, ProcedureCache>> =
                    serde_yaml::from_reader(&file)?;
                let proc_caches = map
                    .remove("proc_caches")
                    .ok_or_else::<serde_yaml::Error, _>(|| {
                        serde::de::Error::missing_field("proc_caches")
                    })?;
                if let Some((key, _)) = map.drain().next() {
                    return Err(serde_yaml::Error::from(serde::de::Error::custom(format!(
                        "unexpected extra field `{}`",
                        key
                    )))
                    .into());
                }

                Ok(Self { proc_caches, file })
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let file = File::create(&context_dir_path)?;
                let context = Self {
                    proc_caches: Default::default(),
                    file,
                };
                serde_yaml::to_writer(&context.file, &context)?;
                Ok(context)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn get_context_dir() -> Result<PathBuf> {
        let home = env::var(ENV_HOME).map_err(|err| Error::environment(ENV_HOME, err))?;
        let mut context_path = PathBuf::new();
        context_path.push(home);
        context_path.push(Self::CONTEXT_DIR);

        Ok(context_path)
    }

    pub fn get_work_dir() -> Result<PathBuf> {
        let mut path = Self::get_context_dir()?;
        path.push(Self::WORK_DIR);
        Ok(path)
    }

    pub fn is_procedure_cached(&self, name: &str) -> bool {
        self.proc_caches.contains_key(name)
    }

    pub fn update_procedure_cache(&mut self, name: String, cache: ProcedureCache) {
        self.proc_caches.insert(name, cache);
    }

    pub fn persist(&mut self) -> Result<()> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        serde_yaml::to_writer(&self.file, self)?;

        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProcedureCache {
    completed_steps: Vec<ProcedureStep>,
    current_step: Option<ProcedureStep>,
}

impl ProcedureCache {
    pub fn new<S: ProcedureState>() -> Result<Self> {
        Ok(Self {
            completed_steps: Vec::new(),
            current_step: Some(ProcedureStep::new(&S::initial_state())?),
        })
    }

    pub fn completed_steps(&self) -> impl Iterator<Item = &ProcedureStep> + '_ {
        self.completed_steps.iter()
    }

    pub fn current_step(&self) -> Option<&ProcedureStep> {
        self.current_step.as_ref()
    }

    pub fn current_step_mut(&mut self) -> Option<&mut ProcedureStep> {
        self.current_step.as_mut()
    }

    pub fn current_state<S: ProcedureState>(&self) -> Result<Option<S>> {
        self.current_step
            .as_ref()
            .map(|s| Ok(s.state()?))
            .transpose()
    }

    pub fn advance<S: ProcedureState>(&mut self, state: &Option<S>) -> Result<()> {
        if let Some(state) = state {
            if let Some(current_step) = self.current_step.take() {
                let proc_step = ProcedureStep::new(state)?;

                self.completed_steps.push(current_step);
                self.current_step.replace(proc_step);
            }
        } else if let Some(current_step) = self.current_step.take() {
            self.completed_steps.push(current_step);
        }

        Ok(())
    }

    pub fn invalidate_state<S: ProcedureState>(&mut self, id: S::Id) -> Result<()> {
        for (index, step) in self.completed_steps.iter().enumerate() {
            if step.id::<S>()? == id {
                self.completed_steps.truncate(index + 1);
                self.current_step
                    .replace(self.completed_steps.remove(index));
                break;
            }
        }

        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProcedureStep {
    id: u64,
    state: String,
    work_dir_state: DirectoryState,
}

impl ProcedureStep {
    fn new<S: ProcedureState>(state: &S) -> Result<Self> {
        Ok(Self {
            id: state.id().to_hash(),
            state: serde_json::to_string(&state)?,
            work_dir_state: DirectoryState::new(&OsStr::new(Context::WORK_DIR)),
        })
    }

    pub fn id<S: ProcedureState>(&self) -> Result<S::Id> {
        S::Id::from_hash(self.id)
    }

    pub fn state<S: ProcedureState>(&self) -> Result<S> {
        Ok(serde_json::from_str(&self.state)?)
    }

    pub fn file_writer<P: AsRef<Path>>(&mut self, path: P) -> Result<FileWriter> {
        let mut actual_path = Context::get_work_dir()?;
        actual_path.push(Context::FILES_DIR);
        actual_path.extend(path.as_ref().iter());

        self.work_dir_state.file_writer(path, &actual_path)
    }
}
