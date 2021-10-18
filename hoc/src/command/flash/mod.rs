use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use structopt::StructOpt;
use strum::{EnumDiscriminants, EnumString, IntoStaticStr};

use crate::{
    procedure::{Halt, Procedure, ProcedureState, ProcedureStateId, ProcedureStep},
    Result,
};
use hoclog::{bail, choose, info, prompt, status, LogErr};

mod decompress;
mod download;
mod flash;
mod modify;
mod util;

#[derive(StructOpt)]
pub struct Flash {
    #[structopt(long)]
    redownload: bool,
}

impl Procedure for Flash {
    type State = FlashState;
    const NAME: &'static str = "flash";

    fn rewind_state(&self) -> Option<FlashStateId> {
        self.redownload.then(|| FlashStateId::Download)
    }

    fn run(&mut self, step: &mut ProcedureStep) -> Result<Halt<FlashState>> {
        let halt = match step.state()? {
            FlashState::Download => self.download(step)?,
            FlashState::Decompress { archive_path } => self.decompress(step, archive_path)?,
            FlashState::Modify { image_path } => self.modify(step, image_path)?,
            FlashState::Flash { image_path } => self.flash(step, image_path)?,
        };

        Ok(halt)
    }
}

#[derive(Debug, Serialize, Deserialize, EnumDiscriminants)]
#[strum_discriminants(derive(Hash, PartialOrd, Ord, EnumString, IntoStaticStr))]
#[strum_discriminants(name(FlashStateId))]
pub enum FlashState {
    Download,
    Decompress { archive_path: PathBuf },
    Modify { image_path: PathBuf },
    Flash { image_path: PathBuf },
}

impl ProcedureStateId for FlashStateId {
    type DeserializeError = strum::ParseError;

    fn description(&self) -> &'static str {
        match self {
            Self::Download => "Download operating system image",
            Self::Decompress => "Decompress image archive",
            Self::Modify => "Modify image",
            Self::Flash => "Flash image",
        }
    }
}

impl Default for FlashState {
    fn default() -> Self {
        FlashState::Download
    }
}

impl ProcedureState for FlashState {
    type Id = FlashStateId;

    fn id(&self) -> Self::Id {
        self.into()
    }
}
