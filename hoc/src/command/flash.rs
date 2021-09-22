use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};
use structopt::StructOpt;
use strum::{EnumIter, IntoEnumIterator};

use crate::{
    file_ref::FileRef,
    procedure::{Halt, Procedure, ProcedureState},
    Result,
};
use hoclog::{choose, error, info, warning};

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

    fn run(&mut self, state: FlashState) -> Result<Halt<FlashState>> {
        match state {
            FlashState::Download => self.download(),
            FlashState::Flash { image } => self.flash(image),
        }
    }
}

impl Flash {
    fn download(&self) -> Result<Halt<FlashState>> {
        let index = choose!(
            "Which image do you want to use?",
            items = Image::iter().map(|i| i.description()),
        );

        let image = Image::iter().nth(index).unwrap();
        info!("Image: {}", image);
        info!("URL  : {}", image.url());

        Ok(Halt::Yield(FlashState::Flash {
            image: FileRef::new("test"),
        }))
    }

    fn flash(&self, image: FileRef) -> Result<Halt<FlashState>> {
        info!("flashing {}", image.path().display());
        warning!("flash warning")?;
        if self.fail_flash {
            error!("flash error")?;
        }

        Ok(Halt::Finish)
    }
}

#[derive(Serialize, Deserialize)]
pub enum FlashState {
    Download,
    Flash { image: FileRef },
}

impl ProcedureState for FlashState {
    type Procedure = Flash;

    fn initial_state() -> Self {
        Self::Download
    }

    fn description(&self) -> &'static str {
        match self {
            Self::Download => "Download operating system image",
            Self::Flash { .. } => "Flash memory card",
        }
    }

    fn needs_update(&self, flash: &Flash) -> Result<bool> {
        let needs_update = match self {
            Self::Download => flash.redownload,
            Self::Flash { image } => flash.reflash || !image.exists()?,
        };

        Ok(needs_update)
    }
}
