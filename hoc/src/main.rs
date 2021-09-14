use std::{
    collections::HashMap,
    env::{self, VarError},
    fs, io,
    path::PathBuf,
    result::Result as StdResult,
};

use structopt::StructOpt;
use thiserror::Error;

use hoclog::{error, info, status, warning};

const ENV_HOME: &str = "HOME";
const CONTEXT_DIR: &str = ".hoc";

#[derive(Debug, Error)]
enum Error {
    #[error("failed to retrieve value for environment variable `{name}`: {source}")]
    Environment {
        name: &'static str,
        source: VarError,
    },

    #[error("procedure `{name}` failed: {source}")]
    Procedure {
        name: &'static str,
        source: ProcedureError,
    },

    #[error(transparent)]
    Io(#[from] io::Error),
}

impl Error {
    fn environment(name: &'static str, source: VarError) -> Self {
        Self::Environment { name, source }
    }

    fn procedure<P: Procedure>(proc: P, source: ProcedureError) -> Self {
        Self::Procedure {
            name: proc.name(),
            source,
        }
    }
}

#[derive(Debug, Error)]
enum ProcedureError {
    #[error("`Start` state can not be returned from procedures")]
    RunStateStartReturned,
}

type Result<T> = StdResult<T, Error>;

trait Procedure {
    type Output;
    type State: ProcedureState;

    fn name(&self) -> &'static str;
    fn run(
        &mut self,
        run: Run<Self::State, Self::Output>,
    ) -> Result<Run<Self::State, Self::Output>>;
}

enum Run<S, O> {
    Start,
    Yield(S),
    Finish(O),
}

impl<S, O> Run<S, O> {
    fn start() -> Self {
        Self::Start
    }

    fn yield_state(self, state: S) -> Self {
        Self::Yield(state)
    }

    fn finish(self, output: O) -> Self {
        Self::Finish(output)
    }
}

struct Context {
    proc_states: HashMap<&'static str, Box<dyn ProcedureState>>,
}

impl Context {
    fn new() -> Result<Self> {
        let home = env::var(ENV_HOME).map_err(|err| Error::environment(ENV_HOME, err))?;
        let mut context_path = PathBuf::new();
        context_path.push(home);
        context_path.push(CONTEXT_DIR);

        match fs::metadata(&context_path) {
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(context_path)?,
            Err(error) => return Err(error.into()),
        }

        Ok(Self {
            proc_states: Default::default(),
        })
    }

    fn run_procedure<P: Procedure>(&mut self, mut proc: P) -> Result<P::Output> {
        let mut run = Run::start();
        let output = loop {
            let state = match proc.run(run)? {
                Run::Yield(state) => state,
                Run::Finish(output) => break output,
                Run::Start => {
                    return Err(Error::procedure(
                        proc,
                        ProcedureError::RunStateStartReturned,
                    ))
                }
            };
            run = Run::Yield(state)
        };

        Ok(output)
    }
}

trait ProcedureState {}

#[derive(StructOpt)]
enum Command {
    Flash(Flash),
}

#[derive(StructOpt)]
struct Flash {}

enum FlashState {}

impl ProcedureState for FlashState {}

impl Procedure for Flash {
    type Output = ();
    type State = FlashState;

    fn name(&self) -> &'static str {
        "flash"
    }

    fn run(
        &mut self,
        run: Run<Self::State, Self::Output>,
    ) -> Result<Run<Self::State, Self::Output>> {
        status!("Level one" => {
            status!("Level two" => {
                info!("Info");
                warning!("Warning");
            });
            info!("Info");
            warning!("Warning");
        });
        info!("Info");
        info!("Info");
        warning!("Warning");
        error!("Error");
        status!("Level one" => ());
        warning!("Warning");
        error!("Error");
        status!("Level one" => ());
        error!("Error");
        status!("Level one" => ());
        status!("Level one" => ());
        Ok(run.finish(()))
    }
}

fn main() {
    let wrapper = || -> Result<()> {
        let mut context = Context::new()?;

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
