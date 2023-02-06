use anyhow::Error;

use crate::{command::util::Helm, prelude::*};

#[throws(Error)]
pub fn run() {
    deploy_storage()?;
}

#[throws(Error)]
fn deploy_storage() {
    progress!("Deploying storage");

    let shell = shell!().start()?;
    shell.run(Helm.repo().update())?;
    shell.run(
        Helm.upgrade("longhorn", "longhorn/longhorn")
            .namespace("longhorn-system")
            .create_namespace()
            .version("1.4.0")
            .set("defaultSettings.createDefaultDiskLabeledNodes", "true"),
    )?;
}
