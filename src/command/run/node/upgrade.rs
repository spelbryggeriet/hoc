use std::path::Path;

use anyhow::Error;
use lazy_regex::regex;

use crate::{
    prelude::*,
    process,
    util::{self, DiskPartitionInfo},
};

#[throws(Error)]
pub fn run(node_name: String, inline_upgrade: bool) {
    if !inline_upgrade {
        check_node(&node_name)?;
    }

    process::global_settings().remote_mode(node_name.clone());

    set_up_ssh()?;
    copying_scripts()?;
    mount_storage()?;

    if !inline_upgrade {
        report(&node_name);
    }
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
fn set_up_ssh() -> bool {
    progress!("Setting up SSH settings");

    let did_change = write_file(
        include_str!("../../../../config/ssh/00-hoc.conf"),
        "/etc/ssh/sshd_config.d/00-hoc.conf",
        None,
    )?;

    if did_change {
        process!(sudo "service ssh restart").run()?;
    }

    did_change
}

#[throws(Error)]
fn copying_scripts() -> bool {
    progress!("Copying scripts");

    write_file(
        include_str!("../../../../config/scripts/shutdown-node.sh"),
        "/usr/local/bin/shutdown-node.sh",
        Some(0o755),
    )?
}

#[throws(Error)]
fn mount_storage() -> bool {
    const MOUNT_DIR: &str = "/media";

    let partitions = find_attached_storage()?;
    if partitions.is_empty() {
        return false;
    }

    progress!("Mounting permanent storage");

    let opt = select!("Which storage do you want to mount?")
        .with_options(partitions)
        .get()?;

    let output = process!("blkid /dev/{id}", id = opt.id).run()?;
    let uuid = regex!(r#"^.*UUID="([^"]*)".*$"#)
        .captures(output.stdout.trim())
        .context("output should contain UUID")?
        .get(1)
        .context("UUID should be first match")?
        .as_str();

    let previous_fstab = process!("cat /etc/fstab").run()?.stdout;
    if previous_fstab
        .lines()
        .find(|line| line.contains(uuid))
        .filter(|line| line.contains(MOUNT_DIR))
        .is_some()
    {
        return false;
    }

    let fstab_line = format!("UUID={uuid} {MOUNT_DIR} auto nosuid,nodev,nofail 0 0");
    let fstab_line = fstab_line.replace('/', r"\/");
    let fstab_sed = format!(
        "/^UUID={uuid}/{{h;s/^UUID={uuid}.*$/{fstab_line}/}};${{x;/^$/{{s//{fstab_line}/;H}};x}}"
    );

    process!(sudo "sed -i '{fstab_sed}' /etc/fstab")
        .revertible(process!(sudo "tee /etc/fstab" <("{previous_fstab}")))
        .run()?;
    let output = process!("cat /etc/fstab").run()?;

    debug!("{}", output.stdout);

    process!(sudo "mount --source /dev/{id}", id = opt.id).run()?;

    true
}

#[throws(Error)]
fn write_file(content: &str, path: impl AsRef<Path>, permissions: Option<u32>) -> bool {
    let path = path.as_ref();

    if file_exists(path)? {
        return false;
    }

    if let Some(dir) = path.parent() {
        process!(sudo "mkdir -p {dir}", dir = dir.to_string_lossy()).run()?;
    }

    let path_str = path.to_string_lossy();
    process!(sudo "tee {path_str}" < ("{content}")).run()?;

    if let Some(permissions) = permissions {
        process!(sudo "chmod {permissions:04o} {path_str}").run()?;
    }

    true
}

#[throws(Error)]
fn file_exists(path: &Path) -> bool {
    progress!(Debug, "Checking file existance");
    debug!("Path: {path:?}");

    let output = process!(sudo "test -e {path}", path = path.to_string_lossy())
        .success_codes([0, 1])
        .run()?;
    output.code == 0
}

#[throws(Error)]
fn find_attached_storage() -> Vec<DiskPartitionInfo> {
    progress!("Finding attached storage");

    util::get_attached_disks()?
        .into_iter()
        .filter(|disk| {
            disk.partitions
                .iter()
                .all(|part| part.name != "system-boot")
        })
        .flat_map(|disk| disk.partitions)
        .collect()
}

fn report(node_name: &str) {
    info!("{node_name} has been successfully upgraded");
}
