use std::net::IpAddr;

use anyhow::{anyhow, Error};

use crate::{prelude::*, process, util::Opt};

#[throws(Error)]
pub fn run(node_name: String) {
    check_not_initialized(&node_name)?;

    let ip_address = get_node_ip_address(&node_name)?;
    if ping_endpoint(ip_address)? == 2 {
        await_node_startup(&node_name, ip_address)?;
    }

    process::global_settings().remote_mode(node_name);

    await_node_preinitialization()?;
    change_password()?;
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
fn ping_endpoint(ip_address: IpAddr) -> i32 {
    progress!("Pinging node");

    process!("ping -o -t 15 -i 5 {ip_address}")
        .success_codes([0, 2])
        .run()?
        .code
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

        if ping_endpoint(ip_address)? == 2 {
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
    process!(sudo "chpasswd" <("{username}:{password}")).run()?
}
