use std::net::IpAddr;

use clap::{CommandFactory, Parser, Subcommand};

use crate::{cidr::Cidr, prelude::*};
pub use run::*;

mod run;

#[macro_use]
mod macros;

actions_summary! {
    init {
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
}

#[derive(Subcommand)]
pub enum Action {
    /// Debug this tool
    #[cfg(debug_assertions)]
    Debug,

    Init(InitAction),
    Deploy(DeployAction),
    Node(NodeAction),
    SdCard(SdCardAction),
}

impl Action {
    #[throws(anyhow::Error)]
    pub async fn run(self) {
        match self {
            Action::Init(init_action) => {
                diagnostics!(InitAction);

                let node_addresses = get_arg!(init_action.node_addresses, default = init).await?;
                let gateway = get_arg!(init_action.gateway, default = init).await?;

                debug!("Checking gateway");
                ensure!(
                    node_addresses.contains(gateway),
                    "gateway IP address `{gateway}` is outside of the subnet mask `/{}`",
                    node_addresses.prefix_len
                );

                let admin_username = get_arg!(init_action.admin_username).await?;

                init::run(node_addresses, gateway, admin_username).await?;
            }
            _ => (),
        }
    }
}

/// Initialize a cluster
#[derive(Parser)]
#[clap(name = "init")]
pub struct InitAction {
    #[clap(
        help = help::init::gateway(),
        long,
        default_missing_value = default::init::gateway(),
        default_value_if("defaults", None, Some(default::init::gateway())),
    )]
    gateway: Option<IpAddr>,

    #[clap(
        help = help::init::node_addresses(),
        long_help = long_help::init::node_addresses(),
        long,
        default_missing_value = default::init::node_addresses(),
        default_value_if("defaults", None, Some(default::init::node_addresses())),
    )]
    node_addresses: Option<Cidr>,

    #[clap(help = help::init::admin_username(), long)]
    admin_username: Option<String>,

    /// Skip prompts for fields that have defaults
    ///
    /// This is equivalent to providing all defaultable flags without a value.
    #[clap(short, long)]
    defaults: bool,
}

/// Deploy an application
#[derive(Parser)]
pub struct DeployAction {}

/// Manage a node
#[derive(Parser)]
pub struct NodeAction {}

/// Manage an SD card
#[derive(Parser)]
pub struct SdCardAction {}
