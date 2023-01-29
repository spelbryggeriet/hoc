use anyhow::Error;

use crate::{prelude::*, util::Opt};

#[throws(Error)]
pub fn run(node_name: String) {
    check_node(&node_name)?;
    drain_node(&node_name)?;
    shutdown_node(&node_name)?;
    report(&node_name)?;
}

#[throws(Error)]
fn check_node(node_name: &str) {
    progress!("Checking node");

    if !kv!("nodes/{node_name}").exists() {
        error!("{node_name} is not a known prepared node name");
        info!(
            "Run the following to prepare an SD card for a node:\
             \n\
             \n  hoc sd-card prepare\
             \n "
        );
        bail!("Failed to deploy {node_name}");
    }

    if !kv!("nodes/{node_name}/initialized")
        .get()?
        .convert::<bool>()?
    {
        bail!(
            "{node_name} has not been deployed. Run the following to deploy the node:\
             \n\
             \n  hoc node deploy {node_name}\
             \n "
        );
    }

    if !files!("admin/kube/config").exists()? {
        bail!("Could not check status on {node_name} since kubeconfig is missing")
    }

    let output = process!(
        "kubectl get node {node_name} \
                -o=jsonpath='{{.status.conditions[?(@.type==\"Ready\")].status}}'",
    )
    .run()?;

    if output.stdout.trim() != "True" {
        bail!("{node_name} is not in the \"Ready\" state");
    }
}

#[throws(Error)]
fn drain_node(node_name: &str) {
    progress!("Draining node");

    process!("kubectl drain {node_name} --ignore-daemonsets --delete-emptydir-data").run()?;
}

#[throws(Error)]
fn shutdown_node(node_name: &str) {
    progress!("Shutting down node");

    process!(sudo "shutdown-node.sh")
        .remote_mode(node_name.to_owned())
        .run()?;
}

#[throws(Error)]
fn report(node_name: &str) {
    info!(
        "{node_name} is now being shut down. When the node has shut down, remove the SD card, put \
        it in this computer, and run the following command:\
        \n\
        \n  hoc sd-card prepare {node_name} --migrate",
    );

    let opt = select!("Do you want to continue?")
        .with_options([Opt::Yes, Opt::No])
        .get()?;

    if opt == Opt::No {
        throw!(inquire::InquireError::OperationCanceled);
    }
}
