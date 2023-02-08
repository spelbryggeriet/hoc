use anyhow::Error;

use crate::{
    command::util::{self, Helm},
    prelude::*,
};

const STORAGE_DEPLOYMENT_NAME: &str = "longhorn";

#[throws(Error)]
pub fn run() {
    deploy_storage()?;
    wait_on_pods()?;
}

#[throws(Error)]
fn deploy_storage() {
    progress!("Deploying storage");

    let shell = shell!().start()?;
    shell.run(Helm.repo().update())?;
    shell.run(
        Helm.upgrade(STORAGE_DEPLOYMENT_NAME, "longhorn/longhorn")
            .namespace("longhorn-system")
            .create_namespace()
            .version("1.4.0")
            .set("defaultSettings.createDefaultDiskLabeledNodes", "true"),
    )?;
}

#[throws(Error)]
fn wait_on_pods() {
    progress!("Waiting on pods to be ready");

    util::k8s_wait_on_pods(STORAGE_DEPLOYMENT_NAME)?;
}
