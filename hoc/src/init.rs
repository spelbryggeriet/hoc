use std::net::IpAddr;

use clap::Parser;

use crate::{cidr::Cidr, context::Context, prelude::*};

args_summary! {
    gateway {
        default = "172.16.0.1",
        help = "The default gateway for the cluster to use",
    }
    node_addresses {
        default = "172.16.4.0/12",
        help = "The node addresses for the cluster to use",
        long_help = "The IP address denote the starting address of the allocation range, and the \
            prefix length denote the network subnet mask.",
    }
    admin_username {
        help = "The username for the cluster administrator"
    }
}

/// Initialize a cluster
#[derive(Parser)]
#[clap(name = "init")]
pub struct Command {
    #[clap(
        help = help::gateway(),
        long,
        default_missing_value = default::gateway(),
        default_value_if("defaults", None, Some(default::gateway())),
    )]
    gateway: Option<IpAddr>,

    #[clap(
        help = help::node_addresses(),
        long_help = help::long::node_addresses(),
        long,
        default_missing_value = default::node_addresses(),
        default_value_if("defaults", None, Some(default::node_addresses())),
    )]
    node_addresses: Option<Cidr>,

    #[clap(help = help::admin_username(), long)]
    admin_username: Option<String>,

    /// Skip prompts for fields that have defaults
    ///
    /// This is equivalent to providing all defaultable flags without a value.
    #[clap(short, long)]
    defaults: bool,
}

impl Command {
    #[throws(anyhow::Error)]
    pub async fn run(self, context: &mut Context) {
        let node_addresses = prompt_arg_default!(self, node_addresses).await?;
        let gateway = prompt_arg_default!(self, gateway).await?;

        debug!("Checking gateway");
        ensure!(
            node_addresses.contains(gateway),
            "gateway IP address `{gateway}` is outside of the subnet mask `/{}`",
            node_addresses.prefix_len
        );

        let _admin_username = prompt_arg!(self, admin_username).await?;

        let _store_progress = progress!("Storing network information");

        context
            .kv
            .put_value("network/start_address", node_addresses.ip_addr.to_string())
            .await?
            .put_value("network/prefix_len", node_addresses.prefix_len)
            .await?
            .put_value("network/gateway", gateway.to_string())
            .await?;
    }
}
