use anyhow::Error;

use crate::prelude::*;

#[throws(Error)]
pub fn run(node_name: String) {
    check_node(&node_name)?;
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
