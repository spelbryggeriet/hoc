use std::{
    fmt::{self, Display, Formatter},
    io::Read,
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use structopt::StructOpt;
use strum::{EnumDiscriminants, EnumIter, EnumString, IntoEnumIterator, IntoStaticStr};
use zip::ZipArchive;

use crate::{
    context::ProcedureStep,
    procedure::{Halt, Procedure, ProcedureState, ProcedureStateId},
    Result,
};
use hoclog::{bail, choose, info, status, LogErr};

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
}

impl Procedure for Flash {
    type State = FlashState;
    const NAME: &'static str = "flash";

    fn rewind_state(&self) -> Option<FlashStateId> {
        self.redownload.then(|| FlashStateId::Download)
    }

    fn run(&mut self, proc_step: &mut ProcedureStep) -> Result<Halt<FlashState>> {
        match proc_step.state()? {
            FlashState::Download => self.download(proc_step),
            FlashState::Decompress { archive_path } => self.decompress(proc_step, archive_path),
            FlashState::Modify { image_path } => self.modify(proc_step, image_path),
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

        let archive_path = PathBuf::from("image");
        status!("Downloading image" => {
            let mut image_writer = proc_step.file_writer(&archive_path)?;
            reqwest::blocking::get(image.url())?.copy_to(&mut image_writer)?;
            image_writer.finish()?;
        });

        Ok(Halt::Yield(FlashState::Decompress { archive_path }))
    }

    fn decompress(
        &self,
        proc_step: &mut ProcedureStep,
        archive_path: PathBuf,
    ) -> Result<Halt<FlashState>> {
        let archive_data = status!("Reading archive" => {
            let image_reader = proc_step.file_reader(&archive_path)?;

            let mut archive =
                ZipArchive::new(image_reader).log_err(|_| "failed to read Zip archive")?;

            let mut buf = None;
            let archive_len = archive.len();
            for i in 0..archive_len {
                let mut archive_file = archive
                    .by_index(i)
                    .log_err(|_| "failed to lookup image in Zip archive")?;

                if archive_file.is_file() && archive_file.name().ends_with(".img") {
                    info!("Found image at index {} among {} items.", i, archive_len);

                    let mut data = Vec::new();
                    status!("Decompressing image" => {
                        archive_file
                            .read_to_end(&mut data)
                            .log_err(|_| "failed to read image in Zip archive")?;
                        buf.replace(data);
                    });
                    break;
                }
            }

            if let Some(data) = buf {
                data
            } else {
                bail!("image not found within Zip archive");
            }
        });

        status!("Save decompressed image to file" => {
            proc_step.file_writer(&archive_path)?.write_and_finish(&archive_data)?;
        });

        Ok(Halt::Yield(FlashState::Modify {
            image_path: archive_path,
        }))
    }

    fn modify(
        &self,
        _proc_step: &mut ProcedureStep,
        _image_path: PathBuf,
    ) -> Result<Halt<FlashState>> {
        Ok(Halt::Finish)
    }
}

#[derive(Serialize, Deserialize, EnumDiscriminants)]
#[strum_discriminants(derive(Hash, PartialOrd, Ord, EnumString, IntoStaticStr))]
#[strum_discriminants(name(FlashStateId))]
pub enum FlashState {
    Download,
    Decompress { archive_path: PathBuf },
    Modify { image_path: PathBuf },
}

impl ProcedureStateId for FlashStateId {
    type DeserializeError = strum::ParseError;

    fn description(&self) -> &'static str {
        match self {
            Self::Download => "Download operating system image",
            Self::Decompress => "Decompress image archive",
            Self::Modify => "Modify image",
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
