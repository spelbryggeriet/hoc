use std::net::IpAddr;

use clap::{CommandFactory, Parser, Subcommand};

use crate::{cidr::Cidr, prelude::*, util::Secret};
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
            long_help = "The IP address denote the starting address of the allocation range, and \
                the prefix length denote the network subnet mask.",
        }
        admin_username {
            help = "The username for the cluster administrator",
        }
        admin_password {
            help = "The path to a file containing the password for the cluster administrator",
            long_help = "The password will be used as the node user password, as well as the \
                passphrase for the SSH keypair."
        }
    }
}

/// Hosting on command
///
/// `hoc` is a tool for easily deploying and managing your own home network cluster. It keeps track
/// of all the necessary files and configuration for you, so you spend less time on being a system
/// administrator and more time on developing services for your cluster.
///
/// To get started, you first have to run the `init` command to setup cluster parameters, secret
/// keys, local network address allocation, etc.
///
/// To add a node to the cluster, you must first prepare and SD card with the node software, which
/// can be done using the `sd-card prepare` command. Once that is done, the `node deploy` command
/// can be used to add the node to the cluster.
#[derive(Subcommand)]
pub enum Action {
    /// Debug this tool
    #[cfg(debug_assertions)]
    Debug,

    Init(InitAction),

    #[clap(subcommand)]
    SdCard(SdCardAction),

    #[clap(subcommand)]
    Node(NodeAction),

    Deploy(DeployAction),
}

/// Initialize the cluster
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

    #[clap(help = help::init::admin_password(), long_help = long_help::init::admin_password(), long)]
    admin_password: Option<Secret<String>>,

    /// Skip prompts for fields that have defaults
    ///
    /// This is equivalent to providing all defaultable flags without a value.
    #[clap(short, long)]
    defaults: bool,
}

/// Deploy an application
#[derive(Parser)]
pub struct DeployAction {}
///
/// Manage an SD card
#[derive(Subcommand)]
pub enum SdCardAction {
    Prepare(SdCardPrepareAction),
}

/// Prepare an SD card for a node to be deployed
#[derive(Parser)]
pub struct SdCardPrepareAction {}

/// Manage a node
#[derive(Subcommand)]
pub enum NodeAction {
    Deploy(NodeDeployAction),
}

/// Deploy a node
#[derive(Parser)]
pub struct NodeDeployAction {}

impl Action {
    #[throws(anyhow::Error)]
    pub async fn run(self) {
        match self {
            Action::Init(init_action) => {
                diagnostics!(InitAction);

                let node_addresses = get_arg!(init_action.node_addresses, default = init)?;
                let gateway = get_arg!(init_action.gateway, default = init)?;

                debug!("Checking gateway");
                ensure!(
                    node_addresses.contains(gateway),
                    "gateway IP address `{gateway}` is outside of the subnet mask `/{}`",
                    node_addresses.prefix_len
                );

                let admin_username = get_arg!(init_action.admin_username)?;
                let admin_password = get_secret_arg!(init_action.admin_password)?;

                init::run(node_addresses, gateway, admin_username, admin_password).await?;
            }

            Action::SdCard(sd_card_action) => match sd_card_action {
                SdCardAction::Prepare(_sd_card_prepare_action) => {
                    sd_card::prepare::run().await?;
                }
            },

            Action::Node(node_action) => match node_action {
                NodeAction::Deploy(_node_deploy_action) => {}
            },

            Action::Deploy(_deploy_action) => {}

            #[cfg(debug_assertions)]
            Action::Debug => (),
        }
    }
}
