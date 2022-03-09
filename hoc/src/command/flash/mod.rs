use std::{
    fs::{self, File},
    io,
    net::IpAddr,
    path::{Path, PathBuf},
};

use colored::Colorize;
use hocproc::procedure;
use osshkeys::{keys::FingerprintHash, PublicKey, PublicParts};
use structopt::StructOpt;

use hoclib::{attributes, DirState};
use hoclog::{choose, info, prompt, status, LogErr, Result};

use crate::command::util::{disk, image::Image};

use self::util::Cidr;
use super::{CreateUser, DownloadOsImage};

mod util;

procedure! {
    #[derive(StructOpt)]
    pub struct Flash {
        /// The image to flash the SD card with.
        #[structopt(long)]
        image: Image,

        /// The name of the node, which will be used as the host name, for instance.
        #[structopt(long, required_if("image", "ubuntu"))]
        node_name: Option<String>,

        /// The username of the administrator.
        #[structopt(long, required_if("image", "ubuntu"))]
        username: Option<String>,

        /// List of CIDR addresses to attach to the network interface.
        #[structopt(long, required_if("image", "ubuntu"))]
        address: Option<Cidr>,

        /// The default gateway for the network interface.
        #[structopt(long, required_if("image", "ubuntu"))]
        gateway: Option<IpAddr>,
    }

    pub enum FlashState {
        #[procedure(transient)]
        DetermineImage,

        #[procedure(transient)]
        ModifyUbuntuImage { image_path: PathBuf },

        #[procedure(transient, finish)]
        FlashImage { image_path: PathBuf },
    }
}

impl Run for FlashState {
    fn determine_image(proc: &mut Flash, _work_dir_state: &DirState) -> Result<Self> {
        let image_path = DirState::get_path::<DownloadOsImage>(
            &attributes!("Image" => proc.image.to_string()),
            Path::new("image"),
        )?;

        let state = match proc.image {
            Image::RaspberryPiOs { .. } => FlashImage { image_path },
            Image::Ubuntu { .. } => ModifyUbuntuImage { image_path },
        };

        Ok(state)
    }

    fn modify_ubuntu_image(
        proc: &mut Flash,
        _work_dir_state: &DirState,
        image_path: PathBuf,
    ) -> Result<Self> {
        let username = proc.username.as_ref().unwrap().as_str();
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

        let image_temp_path = status!("Copy image to temporary location" => {
            let image_temp_path = DirState::create_temp_file("image")?;

            info!("Destination: {}", image_temp_path.to_string_lossy());
            io::copy(
                &mut File::open(image_path)?,
                &mut File::options().write(true).open(&image_temp_path)?,
            )?;

            image_temp_path
        });

        let (mount_dir, dev_disk_id) = disk::attach_disk(&image_temp_path, "system-boot")?;

        status!("Configure image" => {
            use serde_yaml::{Mapping as Map, Value::Sequence as Seq};

            let user_data_path = mount_dir.join("user-data");
            let node_name = proc.node_name.as_ref().unwrap();

            let mut data_map = Map::new();
            data_map.insert("hostname".into(), node_name.clone().into());
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
            let address = proc.address.unwrap();
            let gateway = proc.gateway.unwrap();

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
                        Seq(vec![address.to_string().into()]),
                    );
                    ethernets_map.insert(
                        if gateway.is_ipv4() {
                            "gateway4".into()
                        } else {
                            "gateway6".into()
                        },
                        gateway.to_string().into(),
                    );
                    ethernets_map.insert("nameservers".into(), {
                        let mut nameservers_map = Map::new();
                        nameservers_map.insert(
                            "addresses".into(),
                            Seq(vec![
                                gateway.to_string().into(),
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

        disk::detach_disk(dev_disk_id)?;

        Ok(FlashImage {
            image_path: image_temp_path,
        })
    }

    fn flash_image(
        _proc: &mut Flash,
        _work_dir_state: &DirState,
        image_path: PathBuf,
    ) -> Result<()> {
        let disk_id = status!("Find mounted SD card" => {
            let mut physical_disk_infos: Vec<_> =
                disk::get_attached_disks([disk::Type::Physical])
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
