use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use structopt::StructOpt;
use xz2::read::XzDecoder;
use zip::ZipArchive;

use hoc_core::kv::WriteStore;
use hoc_log::{bail, error, info, status, LogErr, Result};
use hoc_macros::{Procedure, ProcedureState};

use crate::command::util::os::OperatingSystem;

#[derive(Procedure, StructOpt)]
pub struct DownloadImage {
    #[structopt(long)]
    #[procedure(rewind = DownloadOperatingSystemImage)]
    redownload: bool,

    /// The operaring system to use.
    #[structopt(long)]
    #[procedure(attribute)]
    os: OperatingSystem,
}

#[derive(ProcedureState, Serialize, Deserialize)]
pub enum DownloadImageState {
    Download,

    #[state(finish)]
    DecompressZipArchive,

    #[state(finish)]
    DecompressXzFile,
}

impl Run for DownloadImageState {
    fn download(proc: &mut DownloadImage, registry: &impl WriteStore) -> Result<Self> {
        let file_ref = status!("Download image").on(|| {
            let image_url = proc.os.image_url();
            info!("URL: {}", image_url);

            let os = &proc.os;
            let file_ref = registry.create_file(format!("images/{os}"))?;
            let mut file = File::options()
                .read(false)
                .write(true)
                .create(true)
                .open(file_ref.path())?;

            reqwest::blocking::get(image_url)
                .log_err()?
                .copy_to(&mut file)
                .log_err()?;

            hoc_log::Result::Ok(file_ref)
        })?;

        status!("Determine file type").on(|| {
            let output = cmd_file!(file_ref.path()).run()?.1.to_lowercase();
            if output.contains("zip archive") {
                info!("Zip archive file type detected");
                Ok(DecompressZipArchive)
            } else if output.contains("xz compressed data") {
                info!("XZ compressed data file type detected");
                Ok(DecompressXzFile)
            } else {
                error!("Unsupported file type")?.into()
            }
        })
    }

    fn decompress_zip_archive(proc: &mut DownloadImage, registry: &impl WriteStore) -> Result<()> {
        let (image_data, mut image_file) = status!("Read ZIP archive").on(|| {
            let os = &proc.os;
            let archive_path: PathBuf = registry.get(format!("images/{os}"))?.try_into()?;
            let file = File::options().read(true).write(true).open(&archive_path)?;

            let mut archive = ZipArchive::new(&file).log_err()?;

            let mut buf = None;
            let archive_len = archive.len();
            for i in 0..archive_len {
                let mut archive_file = archive
                    .by_index(i)
                    .log_context("Failed to lookup image in ZIP archive")?;

                if archive_file.is_file() && archive_file.name().ends_with(".img") {
                    info!("Found image at index {} among {} items.", i, archive_len);

                    let mut data = Vec::new();
                    status!("Decompress image").on(|| {
                        archive_file
                            .read_to_end(&mut data)
                            .log_context("Failed to read image in ZIP archive")?;
                        buf.replace(data);

                        hoc_log::Result::Ok(())
                    })?;
                    break;
                }
            }

            if let Some(data) = buf {
                hoc_log::Result::Ok((data, file))
            } else {
                bail!("Image not found within ZIP archive");
            }
        })?;

        status!("Save decompressed image to file").on(|| {
            image_file.seek(SeekFrom::Start(0))?;
            image_file.set_len(0)?;
            image_file.write_all(&image_data)?;

            hoc_log::Result::Ok(())
        })?;

        Ok(())
    }

    fn decompress_xz_file(proc: &mut DownloadImage, registry: &impl WriteStore) -> Result<()> {
        let (image_data, mut image_file) = status!("Read XZ file").on(|| {
            let os = &proc.os;
            let file_path: PathBuf = registry.get(format!("images/{os}"))?.try_into()?;
            let file = File::options().read(true).write(true).open(&file_path)?;

            let mut decompressor = XzDecoder::new(&file);

            let mut buf = Vec::new();
            status!("Decompress image").on(|| {
                decompressor
                    .read_to_end(&mut buf)
                    .log_context("Failed to read image in XZ file")
            })?;

            hoc_log::Result::Ok((buf, file))
        })?;

        status!("Save decompressed image to file").on(|| {
            image_file.seek(SeekFrom::Start(0))?;
            image_file.set_len(0)?;
            image_file.write_all(&image_data)?;

            hoc_log::Result::Ok(())
        })?;

        Ok(())
    }
}
