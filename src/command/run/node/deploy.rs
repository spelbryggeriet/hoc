use std::net::IpAddr;

use anyhow::{anyhow, Error};
use lazy_regex::regex;

use crate::{
    prelude::*,
    process::{self, Output},
    util::{self, DiskPartitionInfo, Opt},
};

#[throws(Error)]
pub fn run(node_name: String) {
    check_not_initialized(&node_name)?;

    let ip_address = get_node_ip_address(&node_name)?;
    if ping_endpoint(ip_address)?.code == 2 {
        await_node_startup(&node_name, ip_address)?;
    }

    process::global_settings().remote_mode(node_name.clone());

    await_node_preinitialization()?;
    change_password()?;
    let partitions = find_attached_storage()?;
    if !partitions.is_empty() {
        mount_storage(partitions)?;
    }

    verify_installation()?;
    report(&node_name)?;
}

#[throws(Error)]
fn check_not_initialized(node_name: &str) {
    let initialized: bool = kv!("nodes/{node_name}/initialized").get()?.convert()?;
    if initialized {
        throw!(anyhow!("{node_name} has already been deployed"));
    }
}

#[throws(Error)]
fn get_node_ip_address(node_name: &str) -> IpAddr {
    kv!("nodes/{node_name}/network/address").get()?.convert()?
}

#[throws(Error)]
fn ping_endpoint(ip_address: IpAddr) -> Output {
    progress!("Pinging node");

    process!("ping -o -t 15 -i 5 {ip_address}")
        .success_codes([0, 2])
        .run()?
}

#[throws(Error)]
fn await_node_startup(node_name: &str, ip_address: IpAddr) {
    let mut message = format!(
        "{node_name} could not be reached at {ip_address}. Make sure the node hardware has been \
        prepared with a flashed SD card, is plugged into the local network with ethernet, and is \
        turned on."
    );

    loop {
        info!("{message}");

        let opt = select!("Do you want to try again?")
            .with_options([Opt::Yes, Opt::No])
            .get()?;

        if opt == Opt::No {
            throw!(inquire::InquireError::OperationCanceled);
        }

        if ping_endpoint(ip_address)?.code == 2 {
            message = format!("{node_name} could not be reached at {ip_address}.");
        } else {
            break;
        }
    }
}

#[throws(Error)]
fn await_node_preinitialization() {
    progress!("Waiting for node pre-initialization to finish");
    process!("cloud-init status --wait").run()?;
}

#[throws(Error)]
fn change_password() {
    progress!("Changing password");

    let username: String = kv!("admin/username").get()?.convert()?;
    let password = process::get_remote_password()?.into_non_secret();
    process!("sudo -kSp '' chpasswd" < ("temporary_password\n{username}:{password}"))
        .revertible(process!(sudo "chpasswd" < ("{username}:temporary_password")))
        .run()?
}

#[throws(Error)]
fn find_attached_storage() -> Vec<DiskPartitionInfo> {
    progress!("Finding attached storage");

    util::get_attached_disks()?
        .into_iter()
        .filter(|disk| {
            disk.partitions
                .iter()
                .all(|part| part.name != "system-boot")
        })
        .flat_map(|disk| disk.partitions)
        .collect()
}

#[throws(Error)]
fn mount_storage(partitions: Vec<DiskPartitionInfo>) {
    progress!("Mounting permanent storage");

    let opt = select!("Which storage do you want to mount?")
        .with_options(partitions)
        .get()?;

    let output = process!("blkid /dev/{id}", id = opt.id).run()?;
    let uuid = regex!(r#"^.*UUID="([^"]*)".*$"#)
        .captures(output.stdout.trim())
        .context("output should contain UUID")?
        .get(1)
        .context("UUID should be first match")?
        .as_str();

    let fstab_line = format!("UUID={uuid} /media auto nosuid,nodev,nofail 0 0");
    let fstab_line = fstab_line.replace('/', r"\/");
    let fstab_sed = format!(
        "/^UUID={uuid}/{{h;s/^UUID={uuid}.*$/{fstab_line}/}};${{x;/^$/{{s//{fstab_line}/;H}};x}}"
    );

    let previous_fstab = process!("cat /etc/fstab").run()?.stdout;
    process!(sudo "sed -i '{fstab_sed}' /etc/fstab")
        .revertible(process!(sudo "tee /etc/fstab" <("{previous_fstab}")))
        .run()?;
    let output = process!("cat /etc/fstab").run()?;

    debug!("{}", output.stdout);
}

#[throws(Error)]
fn verify_installation() {
    progress!("Verifying installation");
    process!(sudo "k3s kubectl get nodes").run()?;
}

#[throws(Error)]
fn report(node_name: &str) {
    kv!("nodes/{node_name}/initialized").update(true)?;
    info!("{node_name} has been successfully deployed");
}
