use std::{
    collections::HashMap,
    env::{self, VarError},
    fs::{self, File, OpenOptions},
    io::{self, Seek, SeekFrom},
    mem,
    path::PathBuf,
    result::Result as StdResult,
};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use structopt::StructOpt;
use thiserror::Error;

use hoclog::{error, info, status};

const ENV_HOME: &str = "HOME";
const CONTEXT_DIR: &str = ".hoc";

#[derive(Debug, Error)]
enum Error {
    #[error("failed to retrieve value for environment variable `{name}`: {source}")]
    Environment {
        name: &'static str,
        source: VarError,
    },

    #[error("context could not be serialized/deserialized: {0}")]
    ContextSerde(#[from] serde_yaml::Error),

    #[error("procedure state could not be serialized/deserialized: {0}")]
    ProcedureStateSerde(#[from] serde_json::Error),

    #[error(transparent)]
    Io(#[from] io::Error),
}

impl Error {
    fn environment(name: &'static str, source: VarError) -> Self {
        Self::Environment { name, source }
    }
}

#[derive(Debug, Error)]
enum ProcedureError {}

type Result<T> = StdResult<T, Error>;

trait Procedure {
    type State: ProcedureState;
    const NAME: &'static str;

    fn run(&mut self, state: Self::State) -> Result<Halt<Self::State>>;
}

enum Halt<S> {
    Yield(S),
    Finish,
}

#[derive(Debug, Serialize, Deserialize)]
struct Context {
    proc_caches: HashMap<String, ProcedureCache>,
}

impl Context {
    const CONTEXT_FILE_NAME: &'static str = "context.yaml";

    fn load() -> Result<Self> {
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

    fn get_context_dir() -> Result<PathBuf> {
        let home = env::var(ENV_HOME).map_err(|err| Error::environment(ENV_HOME, err))?;
        let mut context_path = PathBuf::new();
        context_path.push(home);
        context_path.push(CONTEXT_DIR);

        Ok(context_path)
    }

    fn run_procedure<P: Procedure>(&mut self, mut proc: P) -> Result<()> {
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

trait ProcedureState: Serialize + DeserializeOwned {
    const INITIAL_STATE: Self;

    fn description(&self) -> &'static str;
}

#[derive(StructOpt)]
enum Command {
    Flash(Flash),
}

#[derive(StructOpt)]
struct Flash {}

#[derive(Serialize, Deserialize)]
enum FlashState {
    Download,
    Flash,
}

impl ProcedureState for FlashState {
    const INITIAL_STATE: Self = Self::Download;

    fn description(&self) -> &'static str {
        match self {
            Self::Download => "Download operating system image",
            Self::Flash => "Flash memory card",
        }
    }
}

impl Procedure for Flash {
    type State = FlashState;
    const NAME: &'static str = "flash";

    fn run(&mut self, state: FlashState) -> Result<Halt<FlashState>> {
        match state {
            FlashState::Download => self.download(),
            FlashState::Flash => self.flash(),
        }
    }
}

impl Flash {
    fn download(&self) -> Result<Halt<FlashState>> {
        info!("download");
        Ok(Halt::Yield(FlashState::Flash))
    }

    fn flash(&self) -> Result<Halt<FlashState>> {
        info!("flash");
        error!("flash error");
        std::process::exit(1);

        Ok(Halt::Finish)
    }
}

fn main() {
    let wrapper = || -> Result<()> {
        let mut context = Context::load()?;

        match Command::from_args() {
            Command::Flash(proc) => context.run_procedure(proc)?,
        }
        Ok(())
    };

    match wrapper() {
        Ok(_) => (),
        Err(error) => eprintln!("hoc error: {}", error),
    }
}
