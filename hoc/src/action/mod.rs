use std::net::IpAddr;

use clap::{CommandFactory, Parser, Subcommand};

use crate::{cidr::Cidr, prelude::*};

mod init;

#[cfg(debug_assertions)]
pub mod debug;

#[derive(Subcommand)]
pub enum Action {
    /// Debug this tool
    #[cfg(debug_assertions)]
    Debug,

    Init(InitAction),
    Deploy(DeployCommand),
    Node(NodeCommand),
    SdCard(SdCardCommand),
}

impl Action {
    #[throws(anyhow::Error)]
    pub async fn run(self) {
        match self {
            Action::Init(init_action) => {
                debug!("Running {} action", InitAction::command().get_name(),);

                let node_addresses = prompt_arg_default!(init_action, node_addresses).await?;
                let gateway = prompt_arg_default!(init_action, gateway).await?;

                debug!("Checking gateway");
                ensure!(
                    node_addresses.contains(gateway),
                    "gateway IP address `{gateway}` is outside of the subnet mask `/{}`",
                    node_addresses.prefix_len
                );

                let admin_username = prompt_arg!(init_action, admin_username).await?;

                init::run(node_addresses, gateway, admin_username).await?;
            }
            _ => (),
        }
    }
}

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
pub struct InitAction {
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

/// Deploy an application
#[derive(Parser)]
pub struct DeployCommand {}

/// Manage a node
#[derive(Parser)]
pub struct NodeCommand {}

/// Manage an SD card
#[derive(Parser)]
pub struct SdCardCommand {}
