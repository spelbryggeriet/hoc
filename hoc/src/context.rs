use std::{
    collections::HashMap,
    env,
    fs::{self, File},
    io::{self, Seek, SeekFrom},
    num::NonZeroUsize,
    ops::{Index, IndexMut},
    path::PathBuf,
};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{procedure::ProcedureState, Error, Result};

const ENV_HOME: &str = "HOME";

#[derive(Debug, Serialize, Deserialize)]
pub struct Context {
    proc_caches: HashMap<String, ProcedureCache>,
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
    const CONTEXT_DIR: &'static str = ".hoc";

    pub fn load() -> Result<Self> {
        let mut context_path = Self::get_context_dir()?;

        match fs::metadata(&context_path) {
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&context_path)?,
            Err(error) => return Err(error.into()),
        }

        context_path.push(Self::CONTEXT_FILE_NAME);

        match File::open(&context_path) {
            Ok(file) => Ok(serde_yaml::from_reader(file)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let context = Self {
                    proc_caches: Default::default(),
                };
                serde_yaml::to_writer(File::create(context_path)?, &context)?;
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

    pub fn is_procedure_cached(&self, name: &str) -> bool {
        self.proc_caches.contains_key(name)
    }

    pub fn update_procedure_cache(&mut self, name: String, cache: ProcedureCache) {
        self.proc_caches.insert(name, cache);
    }

    pub fn persist(&self, file: &mut File) -> Result<()> {
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        serde_yaml::to_writer(&*file, self)?;

        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProcedureCache {
    completed_steps: Vec<String>,
    current_step: Option<String>,
}

impl ProcedureCache {
    pub fn new<S: ProcedureState>() -> Result<Self> {
        Ok(Self {
            completed_steps: Vec::new(),
            current_step: Some(serde_json::to_string(&S::initial_state())?),
        })
    }

    pub fn completed_steps<S: DeserializeOwned>(
        &self,
    ) -> impl Iterator<Item = Result<(NonZeroUsize, S)>> + '_ {
        self.completed_steps.iter().enumerate().map(|(i, s)| {
            // Safety: adding 1 to any `usize` value will always result in a non-zero value.
            let index = unsafe { NonZeroUsize::new_unchecked(i + 1) };
            Ok((index, serde_json::from_str(s)?))
        })
    }

    pub fn last_index(&self) -> NonZeroUsize {
        // Safety: `new` and `advance` will assure that either `current_step` or `completed_steps`
        // will contain at least one element.
        unsafe {
            NonZeroUsize::new_unchecked(
                self.completed_steps.len() + self.current_step.as_ref().map(|_| 1).unwrap_or(0),
            )
        }
    }

    pub fn current_state<S: DeserializeOwned>(&self) -> Result<Option<S>> {
        self.current_step
            .as_ref()
            .map(|s| Ok(serde_json::from_str(s)?))
            .transpose()
    }

    pub fn advance<S: ProcedureState>(&mut self, state: &Option<S>) -> Result<()> {
        if let Some(state) = state {
            if let Some(current_step) = self.current_step.take() {
                let proc_step = serde_json::to_string(state)?;

                self.completed_steps.push(current_step);
                self.current_step.replace(proc_step);
            }
        } else if let Some(current_step) = self.current_step.take() {
            self.completed_steps.push(current_step);
        }

        Ok(())
    }

    pub fn invalidate_state<S: ProcedureState>(&mut self, index: NonZeroUsize) -> Result<()> {
        if index.get() == 1 {
            *self = Self::new::<S>()?;
        } else if index <= self.last_index() {
            self.completed_steps.truncate(index.get());
            self.current_step
                .replace(self.completed_steps.remove(index.get() - 1));
        }

        Ok(())
    }
}
