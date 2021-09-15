use std::{
    collections::HashMap,
    env::{self, VarError},
    fs::{self, File, OpenOptions},
    io::{self, Seek, SeekFrom},
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
    proc_states: HashMap<String, String>,
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
                    proc_states: Default::default(),
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

        let mut cache = self.get_procedure_cache::<P::State>(P::NAME, &mut file)?;
        if cache.steps.len() > 1 {
            for proc_step in cache.steps.iter().take(cache.steps.len() - 1) {
                status!(("[CACHED] Skipping step {}: {}", proc_step.step, proc_step.description) => ());
            }
        }

        loop {
            cache = if let ProcedureCache {
                state: Some(inner_state),
                mut steps,
            } = cache
            {
                let proc_step = steps.last().unwrap();
                status!(("Step {}: {}", proc_step.step, inner_state.description()) => {
                    let step = proc_step.step + 1;

                    match proc.run(inner_state)? {
                        Halt::Yield(inner_state) => {
                            let description = inner_state.description();

                            steps.push(ProcedureStepDescription { step, description: description.to_owned()});
                            ProcedureCache { state: Some(inner_state), steps }
                        },
                        Halt::Finish => {
                            let description = String::new();

                            steps.push(ProcedureStepDescription { step, description });
                            ProcedureCache {
                                state: None,
                                steps,
                            }
                        }
                    }
                })
            } else {
                cache
            };

            self.save_state(P::NAME, &cache, &mut file)?;

            if cache.state.is_none() {
                break;
            }
        }

        Ok(())
    }

    fn get_procedure_cache<S: ProcedureState>(
        &mut self,
        name: &'static str,
        file: &mut File,
    ) -> Result<ProcedureCache<S>> {
        if !self.proc_states.contains_key(name) {
            let inner_state = S::INITIAL_STATE;
            let description = inner_state.description();
            let state = ProcedureCache {
                state: Some(inner_state),
                steps: vec![ProcedureStepDescription {
                    step: 1,
                    description: description.to_owned(),
                }],
            };
            self.save_state(name, &state, file)?;
            return Ok(state);
        } else {
            return Ok(serde_json::from_str(&self.proc_states[name])?);
        }
    }

    fn save_state<S: ProcedureState>(
        &mut self,
        name: &'static str,
        state: &ProcedureCache<S>,
        file: &mut File,
    ) -> Result<()> {
        self.proc_states
            .insert(name.to_owned(), serde_json::to_string(&state)?);
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        serde_yaml::to_writer(&*file, self)?;

        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct ProcedureCache<S> {
    state: Option<S>,
    steps: Vec<ProcedureStepDescription>,
}

#[derive(Serialize, Deserialize)]
struct ProcedureStepDescription {
    step: usize,
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
