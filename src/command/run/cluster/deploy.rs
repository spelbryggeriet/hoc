use anyhow::Error;

use crate::{
    command::util::{self, Helm},
    prelude::*,
};

const STORAGE_DEPLOYMENT_NAME: &str = "longhorn";

#[throws(Error)]
pub fn run() {
    // deploy_storage()?;
    // wait_on_pods()?;
    add_disks()?;
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
    shell.exit()?;
}

#[throws(Error)]
fn wait_on_pods() {
    progress!("Waiting on pods to be ready");

    util::k8s_wait_on_pods(STORAGE_DEPLOYMENT_NAME)?;
}

#[throws(Error)]
fn add_disks() {
    progress!("Adding storage disks");

    let output = process!("kubectl get nodes -o name").run()?;
    for node_name in output.stdout.split_whitespace() {
        let node_name = node_name.trim_start_matches("node/");
        let fstab = process!("cat /etc/fstab")
            .remote_mode(node_name.to_owned())
            .run()?
            .stdout;
        let mount_dirs = fstab
            .lines()
            .filter_map(|line| line.split_whitespace().nth(1))
            .filter(|dir| dir.starts_with("/media-"));

        for mount_dir in mount_dirs {
            let disk_name = mount_dir.trim_start_matches('/');

            let shell = shell!().remote_mode(node_name.to_owned()).start()?;

            let mut port_forward = shell.spawn(process!(
                "kubectl port-forward services/longhorn-frontend 8080:http -n longhorn-system"
            ))?;

            shell.run(process!(
                PYTHONPATH="/usr/local/lib/python"
                "python3 - {node_name} {disk_name} {mount_dir}" < ("{program}"),
                program = include_str!("../../../../config/scripts/longhorn_add_disk.py")
            ))?;

            port_forward.interrupt()?;
            port_forward.join()?;

            shell.exit()?;
        }
    }
}
