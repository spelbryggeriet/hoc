use std::{
    fmt::{self, Display, Formatter},
    path::PathBuf,
};

use strum::{EnumIter, IntoEnumIterator};

use super::*;

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

impl Flash {
    pub(super) fn download(
        &self,
        proc_step: &mut ProcedureStep,
    ) -> hoclog::Result<Halt<FlashState>> {
        let index = choose!(
            "Which image do you want to use?",
            items = Image::iter().map(|i| i.description()),
        )?;

        let image = Image::iter().nth(index).unwrap();
        info!("Image: {}", image);
        info!("URL  : {}", image.url());

        let archive_path = PathBuf::from("image");
        status!("Downloading image" => {
            let mut image_writer = proc_step.file_writer(&archive_path).log_err()?;
            reqwest::blocking::get(image.url())
                .log_err()?
                .copy_to(&mut image_writer)
                .log_err()?;
            image_writer.finish().log_err()?;
        });

        Ok(Halt::Yield(FlashState::Decompress { archive_path }))
    }
}
