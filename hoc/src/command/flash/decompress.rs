use std::{io::Read, path::PathBuf};

use zip::ZipArchive;

use super::*;

impl Flash {
    pub(super) fn decompress(
        &self,
        proc_step: &mut ProcedureStep,
        archive_path: PathBuf,
    ) -> hoclog::Result<Halt<FlashState>> {
        let archive_data = status!("Reading archive" => {
            let image_reader = proc_step.file_reader(&archive_path).log_err()?;

            let mut archive = ZipArchive::new(image_reader).log_err()?;

            let mut buf = None;
            let archive_len = archive.len();
            for i in 0..archive_len {
                let mut archive_file = archive
                    .by_index(i)
                    .log_context("Failed to lookup image in Zip archive")?;

                if archive_file.is_file() && archive_file.name().ends_with(".img") {
                    info!("Found image at index {} among {} items.", i, archive_len);

                    let mut data = Vec::new();
                    status!("Decompressing image" => {
                        archive_file
                            .read_to_end(&mut data)
                            .log_context("Failed to read image in Zip archive")?;
                        buf.replace(data);
                    });
                    break;
                }
            }

            if let Some(data) = buf {
                data
            } else {
                bail!("Image not found within Zip archive");
            }
        });

        status!("Save decompressed image to file" => {
            proc_step
                .file_writer(&archive_path)
                .log_context("Failed to open file writer")?
                .write_and_finish(&archive_data)
                .log_context("Failed to write to file")?
        });

        Ok(Halt::Yield(FlashState::Modify {
            image_path: archive_path,
        }))
    }
}
