use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use hocproc::procedure;
use structopt::StructOpt;
use xz2::read::XzDecoder;
use zip::ZipArchive;

use hoclib::{procedure::Procedure, DirState};
use hoclog::{bail, error, info, status, LogErr, Result};

use crate::command::util::{disk, image::Image};

procedure! {
    #[derive(StructOpt)]
    pub struct DownloadOsImage {
        #[procedure(rewind = DownloadOperatingSystemImage)]
        #[structopt(long)]
        redownload: bool,

        /// The image to download.
        #[procedure(attribute)]
        #[structopt(long)]
        image: Image,
    }

    pub enum DownloadOsImageState {
        DownloadOperatingSystemImage,

        #[procedure(maybe_finish)]
        DecompressZipArchive {
            image: Image,
        },

        #[procedure(maybe_finish)]
        DecompressXzFile {
            image: Image,
        },

        #[procedure(finish)]
        ModifyRaspberryPiOsImage,
    }
}

impl Run for DownloadOsImageState {
    fn download_operating_system_image(
        proc: &mut DownloadOsImage,
        work_dir_state: &mut DirState,
    ) -> Result<Self> {
        let file_path = status!("Download image" => {
            let image_url = proc.image.url();
            info!("URL: {}", image_url);

            let file_path = work_dir_state.track_file("image");
            let mut file = File::options()
                .read(false)
                .write(true)
                .create(true)
                .open(&file_path)?;

            reqwest::blocking::get(image_url).log_err()?.copy_to(&mut file).log_err()?;
            file_path
        });

        let state = status!("Determine file type" => {
            let output = cmd_file!(file_path).run()?.1.to_lowercase();
            if output.contains("zip archive") {
                info!("Zip archive file type detected");
                DecompressZipArchive {
                    image: proc.image,
                }
            } else if output.contains("xz compressed data") {
                info!("XZ compressed data file type detected");
                DecompressXzFile { image: proc.image }
            } else {
                error!("Unsupported file type")?.into()
            }
        });

        Ok(state)
    }

    fn decompress_zip_archive(
        proc: &mut DownloadOsImage,
        _work_dir_state: &mut DirState,
        image: Image,
    ) -> Result<Option<Self>> {
        let (image_data, mut image_file) = status!("Read ZIP archive" => {
            let archive_path = DirState::get_path::<DownloadOsImage>(&proc.get_attributes(), Path::new("image"))?;
            let file = File::options()
                .read(true)
                .write(true)
                .open(&archive_path)?;

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
                    status!("Decompress image" => {
                        archive_file
                            .read_to_end(&mut data)
                            .log_context("Failed to read image in ZIP archive")?;
                        buf.replace(data);
                    });
                    break;
                }
            }

            if let Some(data) = buf {
                (data, file)
            } else {
                bail!("Image not found within ZIP archive");
            }
        });

        status!("Save decompressed image to file" => {
            image_file.seek(SeekFrom::Start(0))?;
            image_file.set_len(0)?;
            image_file.write_all(&image_data)?;
        });

        let state = match image {
            Image::RaspberryPiOs { .. } => Some(ModifyRaspberryPiOsImage),
            Image::Ubuntu { .. } => None,
        };

        Ok(state)
    }

    fn decompress_xz_file(
        proc: &mut DownloadOsImage,
        _work_dir_state: &mut DirState,
        image: Image,
    ) -> Result<Option<Self>> {
        let (image_data, mut image_file) = status!("Read XZ file" => {
            let file_path = DirState::get_path::<DownloadOsImage>(&proc.get_attributes(), Path::new("image"))?;
            let file = File::options()
                .read(true)
                .write(true)
                .open(&file_path)?;

            let mut decompressor = XzDecoder::new(&file);

            let mut buf = Vec::new();
            status!("Decompress image" => {
                decompressor
                    .read_to_end(&mut buf)
                    .log_context("Failed to read image in XZ file")?;
            });

            (buf, file)
        });

        status!("Save decompressed image to file" => {
            image_file.seek(SeekFrom::Start(0))?;
            image_file.set_len(0)?;
            image_file.write_all(&image_data)?;
        });

        let state = match image {
            Image::RaspberryPiOs { .. } => Some(ModifyRaspberryPiOsImage),
            Image::Ubuntu { .. } => None,
        };

        Ok(state)
    }

    fn modify_raspberry_pi_os_image(
        proc: &mut DownloadOsImage,
        _work_dir_state: &mut DirState,
    ) -> Result<()> {
        let image_path =
            DirState::get_path::<DownloadOsImage>(&proc.get_attributes(), Path::new("image"))?;
        let (mount_dir, dev_disk_id) = disk::attach_disk(&image_path, "boot")?;

        status!("Configure image" => {
            status!("Create SSH file"=> {
                File::create(mount_dir.join("ssh"))?;
            });
        });

        disk::detach_disk(dev_disk_id)?;

        Ok(())
    }
}
