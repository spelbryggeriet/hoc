use std::{
    collections::HashMap,
    env,
    fs::{self, File, OpenOptions},
    io::{self, Seek, SeekFrom},
    mem,
    path::PathBuf,
};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{
    procedure::{Halt, Procedure, ProcedureState},
    Error, Result,
};
use hoclog::status;

const ENV_HOME: &str = "HOME";

#[derive(Debug, Serialize, Deserialize)]
pub struct Context {
    proc_caches: HashMap<String, ProcedureCache>,
}

impl Context {
    const CONTEXT_FILE_NAME: &'static str = "context.yaml";
    const CONTEXT_DIR: &'static str = ".hoc";

    fn get_context_dir() -> Result<PathBuf> {
        let home = env::var(ENV_HOME).map_err(|err| Error::environment(ENV_HOME, err))?;
        let mut context_path = PathBuf::new();
        context_path.push(home);
        context_path.push(Self::CONTEXT_DIR);

        Ok(context_path)
    }

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

    pub fn run_procedure<P: Procedure>(&mut self, mut proc: P) -> Result<()> {
        let mut context_dir = Context::get_context_dir()?;
        context_dir.push(Context::CONTEXT_FILE_NAME);

        let mut file = OpenOptions::new().write(true).open(context_dir)?;

        if !self.proc_caches.contains_key(P::NAME) {
            let state = P::State::INITIAL_STATE;
            let description = state.description().to_owned();
            self.proc_caches.insert(
                P::NAME.to_string(),
                ProcedureCache::new(
                    &state,
                    ProcedureStepDescription {
                        index: 1,
                        description,
                    },
                )?,
            );
            self.persist(&mut file)?;
        }

        let cache = &self.proc_caches[P::NAME];
        if !cache.first_steps.is_empty() {
            for proc_step in cache.first_steps.iter() {
                status!(("[CACHED] Skipping step {}: {}", proc_step.index, proc_step.description) => ());
            }
        }

        let mut state = cache.deserialize_state::<P::State>()?;
        let mut index = cache.last_step.index;

        loop {
            let cache = self.proc_caches.get_mut(P::NAME).unwrap();
            if let Some(inner_state) = state {
                status!(("Step {}: {}", index, inner_state.description()) => {
                    index += 1;
                    let (description, new_state) = match proc.run(inner_state)? {
                        Halt::Yield(inner_state) => {
                            (inner_state.description().to_owned(), Some(inner_state))
                        }
                        Halt::Finish => (String::new(), None),
                    };

                    state = new_state;
                    cache.serialize_state(&state)?;
                    cache.push_step(ProcedureStepDescription { index, description });
                    self.persist(&mut file)?;

                    if state.is_none(){
                        break;
                    }
                });
            } else {
                break;
            };
        }

        Ok(())
    }

    fn persist(&self, file: &mut File) -> Result<()> {
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        serde_yaml::to_writer(&*file, self)?;

        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ProcedureCache {
    state: Option<String>,
    first_steps: Vec<ProcedureStepDescription>,
    last_step: ProcedureStepDescription,
}

impl ProcedureCache {
    fn new<S: Serialize>(state: &S, step: ProcedureStepDescription) -> Result<Self> {
        Ok(Self {
            state: Some(serde_json::to_string(state)?),
            first_steps: Vec::new(),
            last_step: step,
        })
    }

    fn deserialize_state<S: DeserializeOwned>(&self) -> Result<Option<S>> {
        Ok(self
            .state
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?)
    }

    fn serialize_state<S: Serialize>(&mut self, state: &Option<S>) -> Result<()> {
        self.state = state.as_ref().map(serde_json::to_string).transpose()?;
        Ok(())
    }

    fn push_step(&mut self, step: ProcedureStepDescription) {
        self.first_steps
            .push(mem::replace(&mut self.last_step, step));
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ProcedureStepDescription {
    index: usize,
    description: String,
}
