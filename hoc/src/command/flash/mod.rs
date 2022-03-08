use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    net::IpAddr,
    path::{Path, PathBuf},
};

use colored::Colorize;
use hocproc::procedure;
use osshkeys::{keys::FingerprintHash, PublicKey, PublicParts};
use xz2::read::XzDecoder;
use zip::ZipArchive;

use hoclib::{attributes, cmd_macros, procedure::Procedure, DirState};
use hoclog::{bail, choose, error, info, prompt, status, LogErr, Result};

use self::util::{Cidr, Image};
use super::CreateUser;

cmd_macros!(dd, diskutil, cmd_file => "file", hdiutil, sync);

mod util;

procedure! {
    pub struct Flash {
        #[procedure(rewind = DownloadOperatingSystemImage)]
        #[structopt(long)]
        redownload: bool,

        /// The name of the node, which will be used as the host name, for instance.
        #[structopt(long)]
        node_name: String,

        /// The username of the administrator.
        #[structopt(long)]
        username: String,

        /// List of CIDR addresses to attach to the network interface.
        #[structopt(long)]
        address: Cidr,

        /// The default gateway for the network interface.
        #[structopt(long)]
        gateway: IpAddr,
    }

    pub enum FlashState {
        DownloadOperatingSystemImage,

        DecompressZipArchive {
            image: Image,
        },

        DecompressXzFile {
            image: Image,
        },

        ModifyRaspberryPiOsImage,

        #[procedure(transient)]
        ModifyUbuntuImage,

        #[procedure(transient)]
        #[procedure(finish)]
        FlashImage { image_path: PathBuf },
    }
}

impl Run for Flash {
    fn download_operating_system_image(
        &mut self,
        work_dir_state: &mut DirState,
    ) -> hoclog::Result<FlashState> {
        let images = Image::supported_versions();
        let default_index = images
            .iter()
            .position(|i| *i == Image::default())
            .unwrap_or_default();

        let index = choose!(
            "Which image do you want to use?",
            items = &images,
            default_index = default_index
        )?;

        let image = images[index];
        info!("URL: {}", image.url());

        let file_path = PathBuf::from("image");
        let file_real_path = status!("Download image" => {
            let file_real_path = work_dir_state.track_file(&file_path);
            let mut file = File::options()
                .read(false)
                .write(true)
                .create(true)
                .open(&file_real_path)?;

            reqwest::blocking::get(image.url()).log_err()?.copy_to(&mut file).log_err()?;
            file_real_path
        });

        let state = status!("Determine file type" => {
            let output = cmd_file!(file_real_path).run()?.1.to_lowercase();
            if output.contains("zip archive") {
                info!("Zip archive file type detected");
                DecompressZipArchive {
                    image,
                }
            } else if output.contains("xz compressed data") {
                info!("XZ compressed data file type detected");
                DecompressXzFile { image }
            } else {
                error!("Unsupported file type")?.into()
            }
        });

        Ok(state)
    }

    fn decompress_zip_archive(
        &mut self,
        _work_dir_state: &mut DirState,
        image: Image,
    ) -> Result<FlashState> {
        let (image_data, mut image_file) = status!("Read ZIP archive" => {
            let archive_path = DirState::get_path::<Self>(&self.get_attributes(), Path::new("image"))?;
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
            Image::RaspberryPiOs(_) => ModifyRaspberryPiOsImage,
            Image::Ubuntu(_) => ModifyUbuntuImage,
        };

        Ok(state)
    }

    fn decompress_xz_file(
        &mut self,
        _work_dir_state: &mut DirState,
        image: Image,
    ) -> Result<FlashState> {
        let (image_data, mut image_file) = status!("Read XZ file" => {
            let file_path = DirState::get_path::<Self>(&self.get_attributes(), Path::new("image"))?;
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
            Image::RaspberryPiOs(_) => ModifyRaspberryPiOsImage,
            Image::Ubuntu(_) => ModifyUbuntuImage,
        };

        Ok(state)
    }

    fn modify_raspberry_pi_os_image(
        &mut self,
        _work_dir_state: &mut DirState,
    ) -> Result<FlashState> {
        let image_path = DirState::get_path::<Self>(&self.get_attributes(), Path::new("image"))?;
        let (mount_dir, dev_disk_id) = self.attach_disk(&image_path, "boot")?;

        status!("Configure image" => {
            status!("Create SSH file"=> {
                File::create(mount_dir.join("ssh"))?;
            });
        });

        self.detach_disk(dev_disk_id)?;

        Ok(FlashImage { image_path })
    }

    fn modify_ubuntu_image(&mut self, _work_dir_state: &DirState) -> Result<FlashState> {
        let username = self.username.as_str();
        let pub_key_path = DirState::get_path::<CreateUser>(
            &attributes!("Username" => username),
            Path::new(&format!("ssh/id_{username}_ed25519.pub")),
        )
        .log_context("user not found")?;
        let pub_key = fs::read_to_string(pub_key_path)?;

        info!(
            "SSH public key fingerprint randomart:\n{}",
            PublicKey::from_keystr(&pub_key)
                .log_err()?
                .fingerprint_randomart(FingerprintHash::SHA256)
                .log_err()?
        );

        let image_path = DirState::get_path::<Self>(&self.get_attributes(), Path::new("image"))?;
        let image_temp_path = status!("Copy image to temporary location" => {
            let image_temp_path = DirState::create_temp_file("image")?;

            info!("Destination: {}", image_temp_path.to_string_lossy());
            io::copy(
                &mut File::open(image_path)?,
                &mut File::options().write(true).open(&image_temp_path)?,
            )?;

            image_temp_path
        });

        let (mount_dir, dev_disk_id) = self.attach_disk(&image_temp_path, "system-boot")?;

        status!("Configure image" => {
            use serde_yaml::{Mapping as Map, Value::Sequence as Seq};

            let user_data_path = mount_dir.join("user-data");

            let mut data_map = Map::new();
            data_map.insert("hostname".into(), self.node_name.clone().into());
            data_map.insert("manage_etc_hosts".into(), true.into());
            data_map.insert("package_update".into(), false.into());
            data_map.insert("package_upgrade".into(), false.into());
            data_map.insert("packages".into(), ["whois"].into_iter().collect());
            data_map.insert(
                "users".into(),
                Seq(vec![{
                    let mut users_map = Map::new();
                    users_map.insert("name".into(), username.into());
                    users_map.insert(
                        "groups".into(),
                        [
                            "adm", "dialout", "cdrom", "sudo", "audio", "video", "plugdev",
                            "games", "users", "input", "netdev", "gpio", "i2c", "spi",
                        ]
                        .into_iter()
                        .collect(),
                    );
                    users_map.insert("lock_passwd".into(), true.into());
                    users_map.insert("ssh_authorized_keys".into(), Seq(vec![pub_key.into()]));
                    users_map.insert("sudo".into(), "ALL=(ALL) PASSWD:ALL".into());
                    users_map.insert("system".into(), true.into());
                    users_map.into()
                }]),
            );
            data_map.insert("chpasswd".into(), {
                let mut chpasswd_map = Map::new();
                chpasswd_map.insert(
                    "list".into(),
                    [format!("{username}:temp_password")].into_iter().collect(),
                );
                chpasswd_map.insert("expire".into(), true.into());
                chpasswd_map.into()
            });

            let data = serde_yaml::to_string(&data_map).log_err()?;
            let data = "#cloud-config".to_string() + data.strip_prefix("---").unwrap_or(&data);
            info!(
                "Updating {} with the following configuration:\n{}",
                "/user-data".blue(),
                data
            );
            fs::write(&user_data_path, &data)?;

            let network_config_path = mount_dir.join("network-config");

            let mut network_map = data_map;
            network_map.clear();
            network_map.insert("version".into(), 2_u32.into());
            network_map.insert("ethernets".into(), {
                let mut ethernets_map = Map::new();
                ethernets_map.insert("eth0".into(), {
                    let mut ethernets_map = Map::new();
                    ethernets_map.insert("dhcp4".into(), false.into());
                    ethernets_map.insert("dhcp6".into(), false.into());
                    ethernets_map.insert(
                        "addresses".into(),
                        Seq(vec![self.address.to_string().into()]),
                    );
                    ethernets_map.insert(
                        if self.gateway.is_ipv4() {
                            "gateway4".into()
                        } else {
                            "gateway6".into()
                        },
                        self.gateway.to_string().into(),
                    );
                    ethernets_map.insert("nameservers".into(), {
                        let mut nameservers_map = Map::new();
                        nameservers_map.insert(
                            "addresses".into(),
                            Seq(vec![
                                self.gateway.to_string().into(),
                                "8.8.8.8".into(),
                                "8.8.4.4".into(),
                            ]),
                        );
                        nameservers_map.into()
                    });
                    ethernets_map.into()
                });
                ethernets_map.into()
            });

            let data = serde_yaml::to_string(&network_map).log_err()?;
            let data = data
                .strip_prefix("---\n")
                .map(ToString::to_string)
                .unwrap_or(data);
            info!(
                "Updating {} with the following configuration:\n{}",
                "/network-config".blue(),
                data
            );
            fs::write(&network_config_path, &data)?;
        });

        self.detach_disk(dev_disk_id)?;

        Ok(FlashImage {
            image_path: image_temp_path,
        })
    }

    fn flash_image(&mut self, _work_dir_state: &DirState, image_path: PathBuf) -> Result<()> {
        let disk_id = status!("Find mounted SD card" => {
            let mut physical_disk_infos: Vec<_> =
                util::get_attached_disks([util::DiskType::Physical])
                    .log_context("Failed to get attached disks")?;

            let index = choose!("Choose which disk to flash", items = &physical_disk_infos)?;
            physical_disk_infos.remove(index).id
        });

        let disk_path = PathBuf::from(format!("/dev/{}", disk_id));

        status!("Unmount SD card" => diskutil!("unmountDisk", disk_path).run()?);

        status!("Flash SD card" => {
            prompt!("Do you want to flash target disk '{}'?", disk_id)?;

            dd!(
                "bs=1m",
                format!("if={}", image_path.to_string_lossy()),
                format!("of=/dev/r{disk_id}"),
            )
            .sudo()
            .run()?;

            info!(
                "Image '{}' flashed to target disk '{}'",
                image_path.to_string_lossy(),
                disk_id,
            );
        });

        status!("Unmount image disk" => {
            diskutil!("unmountDisk", disk_path).run()?
        });

        Ok(())
    }
}

impl Flash {
    fn attach_disk(&self, image_path: &Path, partition_name: &str) -> Result<(PathBuf, String)> {
        status!("Attach image as disk" => {
            hdiutil!(
                "attach",
                "-imagekey",
                "diskimage-class=CRawDiskImage",
                "-nomount",
                image_path
            )
            .run()?
        });

        let disk_id = status!("Find attached disk" => {
            let mut attached_disks_info: Vec<_> =
                util::get_attached_disks([util::DiskType::Virtual])
                    .log_context("Failed to get attached disks")?
                    .into_iter()
                    .filter(|adi| adi.partitions.iter().any(|p| p.name == partition_name))
                    .collect();

            let index = choose!(
                "Which disk do you want to use?",
                items = &attached_disks_info,
            )?;

            attached_disks_info
                .remove(index)
                .partitions
                .into_iter()
                .find(|p| p.name == partition_name)
                .unwrap()
                .id
        });

        let dev_disk_id = format!("/dev/{}", disk_id);

        let mount_dir = status!("Mount image disk" => {
            let mount_dir = DirState::create_temp_dir("mounted_image")?;
            diskutil!("mount", "-mountPoint", mount_dir, dev_disk_id).run()?;
            mount_dir
        });

        Ok((mount_dir, dev_disk_id))
    }

    fn detach_disk(&self, dev_disk_id: String) -> Result<()> {
        status!("Sync image disk writes" => sync!().run()?);
        status!("Unmount image disk" => {
            diskutil!("unmountDisk", dev_disk_id).run()?
        });
        status!("Detach image disk" => {
            hdiutil!("detach", dev_disk_id).run()?
        });

        Ok(())
    }
}
