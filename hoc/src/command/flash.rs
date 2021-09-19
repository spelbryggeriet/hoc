use serde::{Deserialize, Serialize};
use structopt::StructOpt;

use crate::{
    procedure::{Halt, Procedure, ProcedureState},
    Result,
};
use hoclog::{error, info};

#[derive(StructOpt)]
pub struct Flash {
    #[structopt(long)]
    redownload: bool,

    #[structopt(long)]
    reflash: bool,

    #[structopt(long)]
    fail_download: bool,

    #[structopt(long)]
    fail_flash: bool,
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
        if self.fail_download {
            error!("download error")?;
        }
        Ok(Halt::Yield(FlashState::Flash))
    }

    fn flash(&self) -> Result<Halt<FlashState>> {
        info!("flash");
        if self.fail_flash {
            error!("flash error")?;
        }

        Ok(Halt::Finish)
    }
}

#[derive(Serialize, Deserialize, Hash)]
pub enum FlashState {
    Download,
    Flash,
}

impl ProcedureState for FlashState {
    type Procedure = Flash;

    const INITIAL_STATE: Self = Self::Download;

    fn description(&self) -> &'static str {
        match self {
            Self::Download => "Download operating system image",
            Self::Flash => "Flash memory card",
        }
    }

    fn needs_update(&self, flash: &Flash) -> bool {
        match self {
            Self::Download => flash.redownload,
            Self::Flash => flash.reflash,
        }
    }
}
