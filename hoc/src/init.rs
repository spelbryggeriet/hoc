use std::net::IpAddr;

use clap::Parser;

use crate::{cidr::Cidr, context::Context, prelude::*};

args_summary! {
    gateway(
        default = "172.16.0.1",
        help = "The default gateway for the cluster to use",
    )
    node_addresses(
        default = "172.16.4.0/12",
        help = "The node addresses for the cluster to use",
        long_help = "The IP address denote the starting address of the allocation range, and the \
            prefix length denote the network subnet mask.",
    )
    admin_username(help = "The username for the cluster administrator")
}

/// Initialize a cluster
#[derive(Parser)]
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
    /// This is equivalent to setting all defaultable flags without a value.
    #[clap(short, long)]
    defaults: bool,
}

impl Command {
    #[throws(anyhow::Error)]
    pub fn run(self, mut context: Context) {
        let node_addresses = arg_get_or_default!(self, node_addresses);
        let gateway = arg_get_or_default!(self, gateway);

        trace!("checking gateway");
        ensure!(
            node_addresses.contains(gateway),
            "gateway IP address `{gateway}` is outside of the subnet mask `/{}`",
            node_addresses.prefix_len
        );

        let admin_username = arg_get!(self, admin_username);

        context
            .kv
            .put_value("network/start_address", node_addresses.ip_addr.to_string())?;
    }
}
