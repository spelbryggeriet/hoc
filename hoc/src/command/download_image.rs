use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use structopt::StructOpt;
use xz2::read::XzDecoder;
use zip::ZipArchive;

use hoc_core::kv::{ReadStore, WriteStore};
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
    fn download(
        proc: &mut DownloadImage,
        proc_registry: &impl WriteStore,
        _global_registry: &impl ReadStore,
    ) -> Result<Self> {
        let file_ref = {
            status!("Download image");

            let image_url = proc.os.image_url();
            info!("URL: {}", image_url);

            let file_ref = proc_registry.create_file("image")?;
            let mut file = File::options()
                .read(false)
                .write(true)
                .create(true)
                .open(file_ref.path())?;

            reqwest::blocking::get(image_url)
                .log_err()?
                .copy_to(&mut file)
                .log_err()?;
            file_ref
        };

        let state = {
            status!("Determine file type");

            let output = cmd_file!(file_ref.path()).run()?.1.to_lowercase();
            if output.contains("zip archive") {
                info!("Zip archive file type detected");
                DecompressZipArchive
            } else if output.contains("xz compressed data") {
                info!("XZ compressed data file type detected");
                DecompressXzFile
            } else {
                error!("Unsupported file type")?.into()
            }
        };

        Ok(state)
    }

    fn decompress_zip_archive(
        _proc: &mut DownloadImage,
        proc_registry: &impl WriteStore,
        _global_registry: &impl ReadStore,
    ) -> Result<()> {
        let (image_data, mut image_file) = {
            status!("Read ZIP archive");

            let archive_path: PathBuf = proc_registry.get("image")?.try_into()?;
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
                    {
                        status!("Decompress image");

                        archive_file
                            .read_to_end(&mut data)
                            .log_context("Failed to read image in ZIP archive")?;
                        buf.replace(data);
                    }
                    break;
                }
            }

            if let Some(data) = buf {
                (data, file)
            } else {
                bail!("Image not found within ZIP archive");
            }
        };

        {
            status!("Save decompressed image to file");

            image_file.seek(SeekFrom::Start(0))?;
            image_file.set_len(0)?;
            image_file.write_all(&image_data)?;
        }

        Ok(())
    }

    fn decompress_xz_file(
        _proc: &mut DownloadImage,
        proc_registry: &impl WriteStore,
        _global_registry: &impl ReadStore,
    ) -> Result<()> {
        let (image_data, mut image_file) = {
            status!("Read XZ file");

            let file_path: PathBuf = proc_registry.get("image")?.try_into()?;
            let file = File::options().read(true).write(true).open(&file_path)?;

            let mut decompressor = XzDecoder::new(&file);

            let mut buf = Vec::new();
            {
                status!("Decompress image");

                decompressor
                    .read_to_end(&mut buf)
                    .log_context("Failed to read image in XZ file")?;
            }

            (buf, file)
        };

        {
            status!("Save decompressed image to file");

            image_file.seek(SeekFrom::Start(0))?;
            image_file.set_len(0)?;
            image_file.write_all(&image_data)?;
        }

        Ok(())
    }
}
