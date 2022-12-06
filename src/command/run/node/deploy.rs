use std::net::IpAddr;

use anyhow::{anyhow, Error};

use crate::prelude::*;

#[throws(Error)]
pub async fn run(node_name: String) {
    check_not_initialized(&node_name).await?;

    let ip_address = get_node_ip_address(&node_name).await?;
    ping_node(ip_address, 1).await?;
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
async fn ping_node(ip_address: IpAddr, timeout: u32) {
    progress!("Pinging node");

    cmd!("ping -o -t {timeout} -i 5 {ip_address}").await?;
}
