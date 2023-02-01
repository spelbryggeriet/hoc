use std::net::IpAddr;

use clap::{CommandFactory, Parser};

use crate::{cidr::Cidr, prelude::*};
pub use run::*;
use util::Defaults;

mod run;
mod util;

#[macro_use]
mod macros;

commands_summary! {
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
        container_registry {
            default = "docker.io/library",
            help = "The prefix for the container registry where images are pulled from",
        }
    }
    node_deploy {
        node_name {
            help = "The name of the node",
        }
    }
    node_upgrade {
        node_name {
            help = "The name of the node",
        }
    }
    deploy {
        timeout {
            default = "5m0s",
            help = "Time to wait for any individual Kubernetes operation",
        }
    }
}

/// Hosting on Command
///
/// `hoc` is a tool for easily deploying and managing your own home network cluster. It keeps track
/// of all the necessary files and configuration for you, so you spend less time on being a system
/// administrator and more time on developing services for your cluster.
///
/// To get started, you first have to run the `init` command to setup cluster parameters, secret
/// keys, local network address allocation, etc.
///
/// To add a node to the cluster, you must first prepare an SD card with the node software, which
/// can be done using the `sd-card prepare` command. Once that is done, the `node deploy` command
/// can be used to add the node to the cluster.
#[derive(clap::Subcommand)]
pub enum Command {
    #[cfg(debug_assertions)]
    #[clap(subcommand)]
    Debug(DebugCommand),

    Version(VersionCommand),

    Upgrade(UpgradeCommand),

    Init(InitCommand),

    #[clap(subcommand)]
    SdCard(SdCardCommand),

    #[clap(subcommand)]
    Node(NodeCommand),

    Deploy(DeployCommand),
}

/// Debug functions
#[cfg(debug_assertions)]
#[derive(clap::Subcommand)]
pub enum DebugCommand {
    Progress(DebugProgressCommand),
    Prompt(DebugProgressCommand),
}

/// Debug the progress module
#[cfg(debug_assertions)]
#[derive(Parser)]
#[clap(name = "debug-progress")]
pub struct DebugProgressCommand;

/// Debug the prompt module
#[cfg(debug_assertions)]
#[derive(Parser)]
#[clap(name = "debug-prompt")]
pub struct DebugPromptCommand;

/// Show the current version
#[derive(Parser)]
#[clap(name = "version")]
pub struct VersionCommand;

/// Upgrade `hoc`
#[derive(Parser)]
#[clap(name = "upgrade")]
pub struct UpgradeCommand {
    /// The git ref to compile the source code from
    #[clap(long)]
    from_ref: Option<String>,
}

/// Initialize the cluster
#[derive(Parser)]
#[clap(name = "init")]
pub struct InitCommand {
    #[clap(
        help = help::init::gateway(),
        long,
        default_missing_value = default::init::gateway(),
        num_args = 0..=1,
        require_equals = true,
        default_value_if("defaults", "true", Some(default::init::gateway())),
    )]
    gateway: Option<IpAddr>,

    #[clap(
        help = help::init::node_addresses(),
        long_help = long_help::init::node_addresses(),
        long,
        default_missing_value = default::init::node_addresses(),
        num_args = 0..=1,
        require_equals = true,
        default_value_if("defaults", "true", Some(default::init::node_addresses())),
    )]
    node_addresses: Option<Cidr>,

    #[clap(help = help::init::admin_username(), long)]
    admin_username: Option<String>,

    #[clap(help = help::init::admin_password(), long_help = long_help::init::admin_password(), long)]
    admin_password: Option<Secret<String>>,

    #[clap(
        help = help::init::container_registry(),
        long,
        default_missing_value = default::init::container_registry(),
        num_args = 0..=1,
        require_equals = true,
        default_value_if("defaults", "true", Some(default::init::container_registry())),
    )]
    container_registry: Option<String>,

    #[clap(flatten)]
    defaults: Defaults,
}

/// Deploy an application
#[derive(Parser)]
pub struct DeployCommand {
    #[clap(
        help = help::deploy::timeout(),
        long,
        default_value = default::deploy::timeout(),
    )]
    timeout: String,
}

/// Manage an SD card
#[derive(clap::Subcommand)]
pub enum SdCardCommand {
    Prepare(SdCardPrepareCommand),
}

/// Prepare an SD card for a node to be deployed
#[derive(Parser)]
#[clap(name = "sd-card-prepare")]
pub struct SdCardPrepareCommand {}

/// Manage a node
#[derive(clap::Subcommand)]
pub enum NodeCommand {
    Deploy(NodeDeployCommand),
    Upgrade(NodeUpgradeCommand),
}

/// Deploy a node
#[derive(Parser)]
#[clap(name = "node-deploy")]
pub struct NodeDeployCommand {
    #[clap(help = help::node_deploy::node_name())]
    node_name: String,
}

/// Upgrades a node to use the latest features
#[derive(Parser)]
#[clap(name = "node-upgrade")]
pub struct NodeUpgradeCommand {
    #[clap(help = help::node_upgrade::node_name())]
    node_name: String,
}

impl Command {
    #[throws(anyhow::Error)]
    pub fn run(self) {
        use Command::*;

        match self {
            Version(_) => {
                cmd_diagnostics!(VersionCommand);

                version::run();
            }

            Upgrade(upgrade_command) => {
                cmd_diagnostics!(UpgradeCommand);

                let from_ref = upgrade_command.from_ref;
                arg_diagnostics!(from_ref);

                upgrade::run(from_ref)?;
            }

            Init(init_command) => {
                cmd_diagnostics!(InitCommand);

                let node_addresses = get_arg!(init_command.node_addresses, default = init)?;
                let gateway = get_arg!(init_command.gateway, default = init)?;

                debug!("Checking gateway");
                ensure!(
                    node_addresses.contains(gateway),
                    "gateway IP address `{gateway}` is outside of the subnet mask `/{}`",
                    node_addresses.prefix_len
                );

                let admin_username = get_arg!(init_command.admin_username)?;
                let admin_password = get_secret_arg!(init_command.admin_password)?;

                let container_registry = get_arg!(init_command.container_registry, default = init)?;

                init::run(
                    node_addresses,
                    gateway,
                    admin_username,
                    admin_password,
                    container_registry,
                )?;
            }

            SdCard(sd_card_command) => match sd_card_command {
                SdCardCommand::Prepare(_prepare_command) => {
                    cmd_diagnostics!(SdCardPrepareCommand);

                    sd_card::prepare::run()?;
                }
            },

            Node(node_command) => match node_command {
                NodeCommand::Deploy(deploy_command) => {
                    cmd_diagnostics!(NodeDeployCommand);

                    arg_diagnostics!(node_name, deploy_command.node_name);

                    node::deploy::run(deploy_command.node_name)?;
                }
                NodeCommand::Upgrade(upgrade_command) => {
                    cmd_diagnostics!(NodeUpgradeCommand);

                    arg_diagnostics!(node_name, upgrade_command.node_name);

                    node::upgrade::run(upgrade_command.node_name, false)?;
                }
            },

            Deploy(deploy_command) => {
                cmd_diagnostics!(DeployCommand);

                arg_diagnostics!(timeout, deploy_command.timeout);

                deploy::run(deploy_command.timeout)?;
            }

            #[cfg(debug_assertions)]
            Debug(debug_command) => match debug_command {
                DebugCommand::Progress(_progress_command) => {
                    cmd_diagnostics!(DebugProgressCommand);
                    debug::progress::run();
                }

                DebugCommand::Prompt(_prompt_command) => {
                    cmd_diagnostics!(DebugPromptCommand);
                    debug::prompt::run()?;
                }
            },
        }
    }
}
