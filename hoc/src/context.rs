use std::{
    collections::{hash_map::DefaultHasher, HashMap},
    env,
    fs::{self, File},
    hash::{Hash, Hasher},
    io::{self, Seek, SeekFrom},
    mem,
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
    state: Option<String>,
    first_steps: Vec<ProcedureStepDescription>,
    last_step: ProcedureStepDescription,
}

impl ProcedureCache {
    pub fn new<S: ProcedureState>(state: &S) -> Result<Self> {
        let mut hasher = DefaultHasher::new();
        state.hash(&mut hasher);

        Ok(Self {
            state: Some(serde_json::to_string(state)?),
            first_steps: Vec::new(),
            last_step: ProcedureStepDescription {
                state_hash: hasher.finish(),
                index: 1,
                description: S::INITIAL_STATE.description().to_owned(),
            },
        })
    }

    pub fn cached_steps(&self) -> &[ProcedureStepDescription] {
        &self.first_steps
    }

    pub fn current_step(&self) -> &ProcedureStepDescription {
        &self.last_step
    }

    pub fn advance<S: ProcedureState>(&mut self, state: &Option<S>) -> Result<()> {
        let mut hasher = DefaultHasher::new();
        state.hash(&mut hasher);

        let proc_step = ProcedureStepDescription {
            state_hash: hasher.finish(),
            index: self.last_step.index + 1,
            description: state
                .as_ref()
                .map(S::description)
                .unwrap_or_default()
                .to_owned(),
        };

        self.state = state.as_ref().map(serde_json::to_string).transpose()?;
        self.first_steps
            .push(mem::replace(&mut self.last_step, proc_step));
        Ok(())
    }

    pub fn current_state<S: DeserializeOwned>(&self) -> Result<Option<S>> {
        Ok(self
            .state
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProcedureStepDescription {
    pub state_hash: u64,
    pub index: usize,
    pub description: String,
}
