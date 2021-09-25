use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};
use structopt::StructOpt;
use strum::{EnumDiscriminants, EnumIter, EnumString, IntoEnumIterator, IntoStaticStr};

use crate::{
    context::{dir_state::FileRef, ProcedureStep},
    procedure::{Halt, Procedure, ProcedureState, ProcedureStateId},
    Result,
};
use hoclog::{choose, error, info, status, warning};

#[derive(Clone, Copy, EnumIter, Eq, PartialEq)]
enum Image {
    Raspbian2021_05_07,
}

impl Display for Image {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        self.description().fmt(f)
    }
}

impl Image {
    pub const fn description(&self) -> &'static str {
        match self {
            Self::Raspbian2021_05_07 => "Raspbian (2021-05-07)",
        }
    }

    pub const fn url(&self) -> &'static str {
        match self {
            Self::Raspbian2021_05_07 => "https://downloads.raspberrypi.org/raspios_lite_armhf/images/raspios_lite_armhf-2021-05-28/2021-05-07-raspios-buster-armhf-lite.zip",
        }
    }
}

#[derive(StructOpt)]
pub struct Flash {
    #[structopt(long)]
    redownload: bool,

    #[structopt(long)]
    reflash: bool,

    #[structopt(long)]
    fail_flash: bool,
}

impl Procedure for Flash {
    type State = FlashState;
    const NAME: &'static str = "flash";

    fn rewind_state(&self) -> Option<FlashStateId> {
        self.redownload
            .then(|| FlashStateId::Download)
            .or(self.reflash.then(|| FlashStateId::Flash))
    }

    fn run(&mut self, proc_step: &mut ProcedureStep) -> Result<Halt<FlashState>> {
        match proc_step.state()? {
            FlashState::Download => self.download(proc_step),
            FlashState::Flash { image } => self.flash(image),
        }
    }
}

impl Flash {
    fn download(&self, proc_step: &mut ProcedureStep) -> Result<Halt<FlashState>> {
        let index = choose!(
            "Which image do you want to use?",
            items = Image::iter().map(|i| i.description()),
        );

        let image = Image::iter().nth(index).unwrap();
        info!("Image: {}", image);
        info!("URL  : {}", image.url());

        let mut image_writer = proc_step.file_writer(&"image")?;
        status!("Downloading image" => {
            reqwest::blocking::get(image.url())?.copy_to(&mut image_writer)?
        });
        let image_ref = image_writer.finish()?;

        Ok(Halt::Yield(FlashState::Flash { image: image_ref }))
    }

    fn flash(&self, image: FileRef) -> Result<Halt<FlashState>> {
        info!("flashing {}", image);
        warning!("flash warning")?;
        if self.fail_flash {
            error!("flash error")?;
        }

        Ok(Halt::Finish)
    }
}

#[derive(Serialize, Deserialize, EnumDiscriminants)]
#[strum_discriminants(derive(Hash, PartialOrd, Ord, EnumString, IntoStaticStr))]
#[strum_discriminants(name(FlashStateId))]
pub enum FlashState {
    Download,
    Flash { image: FileRef },
}

impl ProcedureStateId for FlashStateId {
    type DeserializeError = strum::ParseError;

    fn description(&self) -> &'static str {
        match self {
            Self::Download => "Download operating system image",
            Self::Flash => "Flash memory card",
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
