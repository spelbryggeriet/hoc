use std::{
    fs::File,
    io::{Read, Write},
    path::PathBuf,
};

use zip::ZipArchive;

use super::*;

impl Flash {
    pub(super) fn decompress(
        &self,
        step: &mut ProcedureStep,
        archive_path: PathBuf,
    ) -> hoclog::Result<Halt<FlashState>> {
        let (archive_data, mut archive_file) = status!("Reading archive", {
            let archive_real_path = step.register_file(&archive_path).log_err()?;
            let file = File::options()
                .read(true)
                .write(true)
                .open(&archive_real_path)
                .log_err()?;

            let mut archive = ZipArchive::new(&file).log_err()?;

            let mut buf = None;
            let archive_len = archive.len();
            for i in 0..archive_len {
                let mut archive_file = archive
                    .by_index(i)
                    .log_context("Failed to lookup image in Zip archive")?;

                if archive_file.is_file() && archive_file.name().ends_with(".img") {
                    info!("Found image at index {} among {} items.", i, archive_len);

                    let mut data = Vec::new();
                    status!("Decompressing image", {
                        archive_file
                            .read_to_end(&mut data)
                            .log_context("Failed to read image in Zip archive")?;
                        buf.replace(data);
                    });
                    break;
                }
            }

            if let Some(data) = buf {
                (data, file)
            } else {
                bail!("Image not found within Zip archive");
            }
        });

        status!("Save decompressed image to file", {
            archive_file.write(&archive_data).log_err()?;
        });

        Ok(Halt::persistent_yield(FlashState::Modify {
            image_path: archive_path,
        }))
    }
}
