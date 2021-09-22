use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};
use structopt::StructOpt;
use strum::{EnumDiscriminants, EnumIter, IntoEnumIterator};

use crate::{
    context::FileRef,
    procedure::{Halt, Procedure, ProcedureState, ProcedureStateId, UpdateInfo},
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

        let image_ref = FileRef::new(&"image")?;
        status!("Downloading image" => {
            reqwest::blocking::get(image.url())?.copy_to(&mut image_ref.writer()?)?
        });

        Ok(Halt::Yield(FlashState::Flash { image: image_ref }))
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

#[derive(Serialize, Deserialize, EnumDiscriminants)]
#[strum_discriminants(derive(Hash, PartialOrd, Ord, EnumIter))]
#[strum_discriminants(name(FlashStateId))]
pub enum FlashState {
    Download,
    Flash { image: FileRef },
}

impl ProcedureStateId for FlashStateId {
    type MemberIter = FlashStateIdIter;

    fn members() -> Self::MemberIter {
        FlashStateId::iter()
    }

    fn description(&self) -> &'static str {
        match self {
            Self::Download => "Download operating system image",
            Self::Flash => "Flash memory card",
        }
    }
}

impl ProcedureState for FlashState {
    type Procedure = Flash;
    type Id = FlashStateId;

    fn initial_state() -> Self {
        Self::Download
    }

    fn id(&self) -> Self::Id {
        self.into()
    }

    fn needs_update(&self, flash: &Self::Procedure) -> Result<Option<UpdateInfo<Self::Id>>> {
        let state_id = match self {
            Self::Download => flash.redownload.then(|| {
                UpdateInfo::user_update(FlashStateId::Download, "re-download was requested")
            }),
            Self::Flash { image } => (!image.exists()?)
                .then(|| {
                    UpdateInfo::invalid_state(
                        FlashStateId::Download,
                        format!("image file '{}' does not exist", image.path().display()),
                    )
                })
                .or(flash.reflash.then(|| {
                    UpdateInfo::user_update(FlashStateId::Flash, "re-flash was requested")
                })),
        };

        Ok(state_id)
    }
}
