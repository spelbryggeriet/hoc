use std::{
    collections::HashMap,
    env,
    fs::{self, File, OpenOptions},
    io::{self, Seek, SeekFrom},
    ops::{Index, IndexMut},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

use crate::{
    procedure::{ProcedureState, ProcedureStep},
    Error, Result,
};

pub mod dir_state;

const ENV_HOME: &str = "HOME";

#[derive(Debug, Serialize)]
pub struct Context {
    #[serde(flatten)]
    caches: HashMap<String, Cache>,

    #[serde(skip_serializing)]
    file: File,
}

impl Index<&str> for Context {
    type Output = Cache;

    fn index(&self, index: &str) -> &Self::Output {
        &self.caches[index]
    }
}

impl IndexMut<&str> for Context {
    fn index_mut(&mut self, index: &str) -> &mut Self::Output {
        self.caches.get_mut(index).unwrap()
    }
}

impl Context {
    pub const CONTEXT_FILE_NAME: &'static str = "context.yaml";
    pub const CONTEXT_DIR: &'static str = ".hoc";
    pub const WORK_DIR: &'static str = "workdir";

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
                let caches: HashMap<String, Cache> = serde_yaml::from_reader(&file)?;
                Ok(Self { caches, file })
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let file = File::create(&context_dir_path)?;
                let context = Self {
                    caches: Default::default(),
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

    pub fn contains_cache(&self, name: &str) -> bool {
        self.caches.contains_key(name)
    }

    pub fn update_cache(&mut self, name: String, cache: Cache) {
        self.caches.insert(name, cache);
    }

    pub fn persist(&mut self) -> Result<()> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        serde_yaml::to_writer(&self.file, self)?;

        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Cache {
    completed_steps: Vec<ProcedureStep>,
    current_step: Option<ProcedureStep>,
}

impl Cache {
    pub fn new<S: ProcedureState>() -> Result<Self> {
        Ok(Self {
            completed_steps: Vec::new(),
            current_step: Some(ProcedureStep::new(&S::default(), Context::get_work_dir()?)?),
        })
    }

    pub fn completed_steps(&self) -> impl DoubleEndedIterator<Item = &ProcedureStep> + '_ {
        self.completed_steps.iter()
    }

    pub fn current_step_mut(&mut self) -> Option<&mut ProcedureStep> {
        self.current_step.as_mut()
    }

    pub fn advance<S: ProcedureState>(&mut self, state: &Option<S>) -> Result<()> {
        if let Some(state) = state {
            if let Some(mut current_step) = self.current_step.take() {
                current_step.save_work_dir_changes()?;

                let step = ProcedureStep::new(state, current_step.work_dir_state().root_path())?;

                self.completed_steps.push(current_step);
                self.current_step.replace(step);
            }
        } else if let Some(mut current_step) = self.current_step.take() {
            current_step.save_work_dir_changes()?;
            self.completed_steps.push(current_step);
        }

        Ok(())
    }

    pub fn invalidate_state<S: ProcedureState>(&mut self, id: S::Id) -> Result<()> {
        for (index, step) in self.completed_steps.iter().enumerate() {
            if step.id::<S>()? == id {
                self.completed_steps.truncate(index + 1);

                let mut current_step = self.completed_steps.remove(index);
                current_step.unregister_path("")?;
                self.current_step.replace(current_step);
                break;
            }
        }

        Ok(())
    }
}
