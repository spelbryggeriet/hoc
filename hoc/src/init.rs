use std::{any::Any, net::IpAddr};

use clap::Parser;
use log::Log;

use crate::{cidr::Cidr, context::Context, logger::Logger, prelude::*};

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
    /// This is equivalent to setting all defaultable flags without a value.
    #[clap(short, long)]
    defaults: bool,
}

impl Command {
    #[throws(anyhow::Error)]
    pub fn run(self, mut context: Context) {
        let node_addresses = arg_get_or_default!(self, node_addresses);
        let gateway = arg_get_or_default!(self, gateway);

        let progress_testing = progress!("Testing progress");

        debug!("Checking gateway");
        ensure!(
            node_addresses.contains(gateway),
            "gateway IP address `{gateway}` is outside of the subnet mask `/{}`",
            node_addresses.prefix_len
        );

        let progress_nested = progress!("Testing nested progress");
        let admin_username = arg_get!(self, admin_username);

        std::thread::sleep(std::time::Duration::new(0, 500_000_000));
        info!("test");
        let progress_nested_nested = progress!("Testing nested progress");
        std::thread::sleep(std::time::Duration::new(2, 500_000_000));
        info!("test");
        std::thread::sleep(std::time::Duration::new(0, 500_000_000));
        progress_nested_nested.finish();
        info!("test");
        std::thread::sleep(std::time::Duration::new(0, 500_000_000));

        progress_nested.finish();
        let _progress_nested_2 = progress!("Testing nested progress 2");

        std::thread::sleep(std::time::Duration::new(1, 500_000_000));

        // context
        // .kv
        // .put_value("network/start_address", node_addresses.ip_addr.to_string())?;
        progress_testing.finish()
    }
}
