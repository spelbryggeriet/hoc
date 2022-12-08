use std::net::IpAddr;

use anyhow::{anyhow, Error};

use crate::{prelude::*, util::Opt};

#[throws(Error)]
pub async fn run(node_name: String) {
    check_not_initialized(&node_name).await?;

    let ip_address = get_node_ip_address(&node_name).await?;
    if ping_endpoint(ip_address, 1).await.is_err() {
        await_node(&node_name, ip_address).await?;
    }
}

#[throws(Error)]
async fn check_not_initialized(node_name: &str) {
    let initialized: bool = kv!("nodes/{node_name}/initialized").await?.convert()?;
    if initialized {
        throw!(anyhow!("{node_name} has already been deployed"));
    }
}

#[throws(Error)]
async fn get_node_ip_address(node_name: &str) -> IpAddr {
    kv!("nodes/{node_name}/network/address").await?.convert()?
}

#[throws(Error)]
async fn ping_endpoint(ip_address: IpAddr, timeout: u32) {
    progress!("Pinging node");

    process!("ping -o -t {timeout} -i 5 {ip_address}").await?;
}

#[throws(Error)]
async fn await_node(node_name: &str, ip_address: IpAddr) {
    info!(
        "{node_name} could not be reached at {ip_address}. Make sure the node hardware has been \
        prepared with a flashed SD card, is plugged into the local network with ethernet, and is \
        turned on."
    );

    let opt = select!("Do you want to continue?")
        .with_options([Opt::Yes, Opt::No])
        .get()?;

    if opt == Opt::No {
        throw!(inquire::InquireError::OperationCanceled);
    }

    ping_endpoint(ip_address, 300).await?;

    progress!("Waiting for node pre-initialization to finish");

    process!("cloud-init status --wait").remote_mode().await?;
}
