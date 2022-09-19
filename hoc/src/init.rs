use std::net::IpAddr;

use anyhow::bail;
use clap::Parser;

use crate::{cidr::Cidr, prelude::*};

args_summary! {
    gateway = "172.16.0.1"
        -> "The default gateway for the cluster to use",
    node_addresses = "172.16.4.0/12"
        -> "The node addresses for the cluster to use"
        -> "The IP address denote the starting address of the allocation range, and the prefix \
            length denote the network subnet mask.",
    admin_username
        -> "The username for the cluster administrator",
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
    #[throws(Error)]
    pub fn run(self) {
        let node_addresses = arg_get_or_default!(self, node_addresses);
        let gateway = arg_get_or_default!(self, gateway);
        let admin_username = arg_get!(self, admin_username);

        if !node_addresses.contains(gateway) {
            bail!(
                "gateway IP address `{gateway}` is outside of the subnet mask `/{}`",
                node_addresses.prefix_len
            );
        }

        ()
    }
}
