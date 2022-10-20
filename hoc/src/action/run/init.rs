use std::net::IpAddr;

use crate::{cidr::Cidr, prelude::*, util::Secret};

#[throws(anyhow::Error)]
pub async fn run(
    node_addresses: Cidr,
    gateway: IpAddr,
    admin_username: String,
    _admin_password: Secret<String>,
) {
    let net_info_progress = progress!("Storing network information");

    put!(node_addresses.ip_addr.to_string() => "network/start_address").await?;
    put!(node_addresses.prefix_len => "network/prefix_len").await?;
    put!(gateway.to_string() => "network/gateway").await?;

    net_info_progress.finish();
    progress_scoped!("Storing administrator user");

    put!(admin_username.to_string() => "admin/username").await?;
}
