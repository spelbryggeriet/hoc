use std::{
    fs::{self, File},
    net::IpAddr,
    path::PathBuf,
};

use colored::Colorize;
use osshkeys::{keys::FingerprintHash, PublicKey, PublicParts};
use serde::{Deserialize, Serialize};
use structopt::StructOpt;

use hoc_core::kv::{ReadStore, WriteStore};
use hoc_log::{choose, error, info, prompt, status, LogErr, Result};
use hoc_macros::{Procedure, ProcedureState};

use crate::command::util::{cidr::Cidr, disk, os::OperatingSystem};

#[derive(Procedure, StructOpt)]
pub struct PrepareSdCard {
    /// Re-flash the image.
    #[structopt(long)]
    #[procedure(rewind = FlashImage)]
    reflash: bool,

    /// The operating system to flash the SD card with.
    #[structopt(long)]
    os: OperatingSystem,

    /// The name of the node.
    #[structopt(long)]
    #[procedure(attribute)]
    node_name: String,

    /// The username of the administrator.
    #[structopt(long, required_if("os", "ubuntu"))]
    username: Option<String>,

    /// List of CIDR addresses to attach to the network interface.
    #[structopt(long, required_if("os", "ubuntu"))]
    address: Option<Cidr>,

    /// The default gateway for the network interface.
    #[structopt(long, required_if("os", "ubuntu"))]
    gateway: Option<IpAddr>,
}

#[derive(ProcedureState, Serialize, Deserialize)]
pub enum PrepareSdCardState {
    FlashImage,

    #[state(transient)]
    Mount,

    #[state(transient)]
    ModifyRaspberryPiOsImage {
        disk_partition_id: String,
    },

    #[state(transient)]
    ModifyUbuntuImage {
        disk_partition_id: String,
    },

    #[state(transient, finish)]
    Unmount {
        disk_partition_id: String,
    },
}

impl Run for PrepareSdCardState {
    fn flash_image(
        proc: &mut PrepareSdCard,
        _proc_registry: &impl WriteStore,
        global_registry: &impl ReadStore,
    ) -> Result<Self> {
        let mut disks: Vec<_> = disk::get_attached_disks()
            .log_context("Failed to get attached disks")?
            .collect();

        let index = if disks.len() == 1 {
            0
        } else {
            choose!("Which disk is your SD card?").items(&disks).get()?
        };

        let disk = disks.remove(index);
        status!("Unmount SD card").on(|| diskutil!("unmountDisk", disk.id).run())?;

        status!("Flash SD card").on(|| {
            let image_path: PathBuf = global_registry
                .get(format!("download-image/{}/image", proc.os))?
                .try_into()?;

            prompt!(
                "Do you want to flash target disk '{}' with operating system '{}'?",
                disk.description(),
                proc.os,
            )?;

            dd!(
                "bs=1m",
                format!("if={}", image_path.to_string_lossy()),
                format!("of=/dev/r{}", disk.id),
            )
            .sudo()
            .run()?;

            hoc_log::Result::Ok(())
        })?;

        Ok(Mount)
    }

    fn mount(
        proc: &mut PrepareSdCard,
        _proc_registry: &impl ReadStore,
        _global_registry: &impl ReadStore,
    ) -> Result<Self> {
        let boot_partition_name = match proc.os {
            OperatingSystem::RaspberryPiOs { .. } => "boot",
            OperatingSystem::Ubuntu { .. } => "system-boot",
        };

        let disk_partition_id = status!("Mount boot partition").on(|| {
            let mut partitions: Vec<_> = disk::get_attached_disk_partitions()
                .log_context("Failed to get attached disks")?
                .filter(|p| p.name == boot_partition_name)
                .collect();

            let index = if partitions.len() == 1 {
                0
            } else {
                choose!("Which refers to the boot partition of the disk?")
                    .items(&partitions)
                    .get()?
            };

            let disk_partition = partitions.remove(index);

            prompt!(
                "Do you want to mount disk partition '{}'?",
                disk_partition.description(),
            )?;

            diskutil!("mount", disk_partition.id).run()?;

            hoc_log::Result::Ok(disk_partition.id)
        })?;

        let state = match proc.os {
            OperatingSystem::RaspberryPiOs { .. } => ModifyRaspberryPiOsImage { disk_partition_id },
            OperatingSystem::Ubuntu { .. } => ModifyUbuntuImage { disk_partition_id },
        };

        Ok(state)
    }

    fn modify_raspberry_pi_os_image(
        _proc: &mut PrepareSdCard,
        _proc_registry: &impl ReadStore,
        _global_registry: &impl ReadStore,
        disk_partition_id: String,
    ) -> Result<Self> {
        let mount_dir = disk::find_mount_dir(&disk_partition_id)?;

        status!("Configure image")
            .on(|| status!("Create SSH file").on(|| File::create(mount_dir.join("ssh"))))?;

        Ok(Unmount { disk_partition_id })
    }

    fn modify_ubuntu_image(
        proc: &mut PrepareSdCard,
        _proc_registry: &impl ReadStore,
        global_registry: &impl ReadStore,
        disk_partition_id: String,
    ) -> Result<Self> {
        let username = proc.username.as_ref().unwrap().as_str();

        let pub_key = status!("Read SSH keypair").on(|| {
            let pub_key_path: PathBuf = global_registry
                .get(format!("create-user/{username}/ssh/id_ed25519.pub"))?
                .try_into()?;

            let pub_key = fs::read_to_string(pub_key_path)?;
            info!(
                "SSH public key fingerprint randomart:\n{}",
                PublicKey::from_keystr(&pub_key)
                    .log_err()?
                    .fingerprint_randomart(FingerprintHash::SHA256)
                    .log_err()?
            );

            hoc_log::Result::Ok(pub_key)
        })?;

        let mount_dir = disk::find_mount_dir(&disk_partition_id)?;

        status!("Prepare image initialization").on(|| {
            let data_map: serde_yaml::Value = serde_yaml::from_str(&format!(
                include_str!("../../config/user-data"),
                admin_username = username,
                hostname = proc.node_name,
                ssh_pub_key = pub_key,
            ))
            .log_context("invalid user-data format")?;

            let data = serde_yaml::to_string(&data_map).log_err()?;
            let data = "#cloud-config".to_string() + data.strip_prefix("---").unwrap_or(&data);
            info!(
                "Updating {} with the following configuration:\n{}",
                "/user-data".blue(),
                data
            );

            let user_data_path = mount_dir.join("user-data");
            fs::write(&user_data_path, &data)?;

            let gateway = proc.gateway.unwrap();
            let data_map: serde_yaml::Value = serde_yaml::from_str(&format!(
                include_str!("../../config/network-config"),
                address = proc.address.unwrap(),
                gateway = gateway,
                gateway_ip_version = if gateway.is_ipv4() {
                    4
                } else if gateway.is_ipv6() {
                    6
                } else {
                    error!("Unspecified gateway IP version")?.into()
                },
            ))
            .log_context("invalid network-config format")?;

            let data = serde_yaml::to_string(&data_map).log_err()?;
            let data = data
                .strip_prefix("---\n")
                .map(ToString::to_string)
                .unwrap_or(data);
            info!(
                "Updating {} with the following configuration:\n{}",
                "/network-config".blue(),
                data
            );

            let network_config_path = mount_dir.join("network-config");
            fs::write(&network_config_path, &data)?;

            hoc_log::Result::Ok(())
        })?;

        Ok(Unmount { disk_partition_id })
    }

    fn unmount(
        _proc: &mut PrepareSdCard,
        _proc_registry: &impl ReadStore,
        _global_registry: &impl ReadStore,
        disk_partition_id: String,
    ) -> Result<()> {
        status!("Sync image disk writes").on(|| sync!().run())?;
        status!("Unmount image disk").on(|| diskutil!("unmount", disk_partition_id).run())?;

        Ok(())
    }
}
