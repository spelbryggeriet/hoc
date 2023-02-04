use anyhow::Error;

use crate::prelude::*;

#[throws(Error)]
pub fn run() {
    deploy_storage()?;
}

#[throws(Error)]
fn deploy_storage() {
    progress!("Deploying storage");

    let shell = shell!().start()?;
    shell.run(process!("helm repo update"))?;
    shell.run(process!(
        "helm install longhorn longhorn/longhorn \
            --namespace longhorn-system \
            --create-namespace \
            --version 1.4.0 \
            --set defaultSettings.createDefaultDiskLabeledNodes=true",
    ))?;
}
