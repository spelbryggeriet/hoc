use std::net::IpAddr;

use crate::{cidr::Cidr, prelude::*};

#[throws(anyhow::Error)]
pub async fn run(node_addresses: Cidr, gateway: IpAddr, _admin_username: String) {
    let _store_progress = progress!("Storing network information");

    put!(node_addresses.ip_addr.to_string() => "network/start_address").await?;
    put!(node_addresses.prefix_len => "network/prefix_len").await?;
    put!(gateway.to_string() => "network/gateway").await?;
}
