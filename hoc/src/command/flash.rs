use serde::{Deserialize, Serialize};
use structopt::StructOpt;

use crate::{
    procedure::{Halt, Procedure, ProcedureState},
    Result,
};
use hoclog::{error, info};

#[derive(StructOpt)]
pub struct Flash {}

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

#[derive(Serialize, Deserialize)]
pub enum FlashState {
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
