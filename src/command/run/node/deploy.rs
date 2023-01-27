use std::{io::Write, net::IpAddr};

use anyhow::Error;
use lazy_regex::regex;

use crate::{
    context::{self, kv},
    prelude::*,
    process,
    util::{self, DiskPartitionInfo, Opt},
};

#[throws(Error)]
pub fn run(node_name: String) {
    check_node(&node_name)?;

    let ip_address = get_node_ip_address(&node_name)?;
    await_node_startup(&node_name, ip_address)?;

    process::global_settings().remote_mode(node_name.clone());

    await_node_initialization()?;
    change_password()?;
    mount_storage()?;
    join_cluster(&node_name)?;
    copy_kubeconfig(ip_address)?;

    verify_installation(&node_name)?;
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

    if kv!("nodes/{node_name}/initialized")
        .get()?
        .convert::<bool>()?
    {
        if !files!("admin/kube/config").exists()? {
            bail!("Could not check status on {node_name} since kubeconfig is missing")
        }

        let output = process!(
            "kubectl get node {node_name} \
                -o=jsonpath='{{.status.conditions[?(@.type==\"Ready\")].status}}'",
        )
        .run()?;

        if output.stdout.trim() == "True" {
            bail!("{node_name} has already been deployed");
        } else {
            bail!("{node_name} has already been deployed, but it is not ready")
        }
    }
}

#[throws(Error)]
fn get_node_ip_address(node_name: &str) -> IpAddr {
    kv!("nodes/{node_name}/network/address").get()?.convert()?
}

#[throws(Error)]
fn ping_endpoint(ip_address: IpAddr) -> bool {
    progress!("Pinging node");

    let shell = shell!().start()?;
    let mut i = 0;
    let reached_endpoint = loop {
        if i == 3 {
            break false;
        }

        let output = shell.run(process!("ping -c 1 {ip_address}").success_codes([0, 2]))?;
        if output.code == 0 {
            return true;
        }

        shell.run(process!("sleep 5"))?;

        i += 1;
    };
    shell.exit()?;
    reached_endpoint
}

#[throws(Error)]
fn await_node_startup(node_name: &str, ip_address: IpAddr) {
    if ping_endpoint(ip_address)? {
        return;
    }

    let mut message = format!(
        "{node_name} could not be reached at {ip_address}. Make sure the node hardware has been \
        prepared with a flashed SD card, is plugged into the local network with ethernet, and is \
        turned on."
    );

    loop {
        info!("{message}");

        let opt = select!("Do you want to try again?")
            .with_options([Opt::Yes, Opt::No])
            .get()?;

        if opt == Opt::No {
            throw!(inquire::InquireError::OperationCanceled);
        }

        if !ping_endpoint(ip_address)? {
            message = format!("{node_name} could not be reached at {ip_address}.");
        } else {
            break;
        }
    }
}

#[throws(Error)]
fn await_node_initialization() {
    let init_status = progress_with_handle!("Checking node initialization status");

    let output = process!("cloud-init status").run()?;
    if output.stdout.contains("status: done") {
        info!("Status: Done");
        return;
    }
    info!("Status: In progress");
    init_status.finish();

    progress!("Waiting for node initialization to finish");
    process!("cloud-init status --wait").run()?;
}

#[throws(Error)]
fn change_password() {
    let check_pass = progress_with_handle!("Checking password");
    let code = process!(sudo "true")
        .sudo_password("temporary_password")
        .success_codes([0, 1])
        .run()?
        .code;
    if code == 1 {
        return;
    }
    check_pass.finish();

    progress!("Changing password");

    let username: String = kv!("admin/username").get()?.convert()?;
    let password = process::get_remote_password()?.into_non_secret();
    process!(sudo "chpasswd" < ("{username}:{password}"))
        .sudo_password("temporary_password")
        .revertible(process!(
            sudo "chpasswd" < ("{username}:temporary_password")
        ))
        .run()?
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

#[throws(Error)]
fn mount_storage() {
    let partitions = find_attached_storage()?;
    if partitions.is_empty() {
        return;
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

    let fstab_line = format!("UUID={uuid} /var/lib/rancher/k3s auto nosuid,nodev,nofail 0 0");
    let fstab_line = fstab_line.replace('/', r"\/");
    let fstab_sed = format!(
        "/^UUID={uuid}/{{h;s/^UUID={uuid}.*$/{fstab_line}/}};${{x;/^$/{{s//{fstab_line}/;H}};x}}"
    );

    let previous_fstab = process!("cat /etc/fstab").run()?.stdout;
    process!(sudo "sed -i '{fstab_sed}' /etc/fstab")
        .revertible(process!(sudo "tee /etc/fstab" <("{previous_fstab}")))
        .run()?;
    let output = process!("cat /etc/fstab").run()?;

    debug!("{}", output.stdout);
}

#[throws(Error)]
fn join_cluster(node_name: &str) {
    progress!("Joining cluster");

    match files!("admin/kube/config").get() {
        Ok(kubeconfig_file) => {
            let kubeconfig: kv::Item = serde_yaml::from_reader(kubeconfig_file)?;
            let k3s_url: String = kubeconfig
                .take("clusters")
                .into_iter()
                .flatten()
                .filter_key_value("name", "default")
                .try_get("cluster/server")
                .and_convert::<String>()
                .next()
                .and_then(Result::ok)
                .context("Could not read server address from the kubeconfig")?;

            let configured_ip_address: IpAddr = k3s_url
                .trim_start_matches("https://")
                .trim_end_matches(":6443")
                .parse()
                .context("Could not parse the server IP address in the kubeconfig")?;
            let self_ip_address: IpAddr =
                kv!("nodes/{node_name}/network/address").get()?.convert()?;

            if self_ip_address == configured_ip_address {
                bail!("Node IP address can not be the same as the host node IP address");
            }

            let configured_node_name = match kv!("nodes/**").get()? {
                kv::Item::Map(map) => map
                    .into_iter()
                    .find_map(|(key, item)| {
                        item.take("network/address")?
                            .convert::<IpAddr>()
                            .ok()
                            .filter(|ip_addr| *ip_addr == configured_ip_address)
                            .map(|_| key)
                    })
                    .context("Could not find server node name in context")?,
                _ => bail!("Could not determine server node name due to invalid context"),
            };

            let output = process!(sudo "cat /var/lib/rancher/k3s/server/node-token")
                .remote_mode(configured_node_name)
                .run()?;
            let k3s_token = output.stdout.trim();

            process!(
                K3S_URL = "{k3s_url}"
                K3S_TOKEN = "{k3s_token}"
                sudo "k3s-init.sh"
            )
            .run()?;
        }
        Err(context::Error::KeyDoesNotExist(_)) => {
            process!(sudo "k3s-init.sh").run()?;
        }
        Err(error) => throw!(error),
    }
}

#[throws(Error)]
fn copy_kubeconfig(ip_address: IpAddr) {
    let check_kubeconf = progress_with_handle!("Checking existing kubeconfig");
    if files!("admin/kube/config").exists()? {
        return;
    }
    info!("Config was not found");
    check_kubeconf.finish();

    progress!("Copying kubeconfig");

    let output = process!(sudo "cat /etc/rancher/k3s/k3s.yaml").run()?;
    let mut kubeconfig_file = files!("admin/kube/config").permissions(0o600).create()?;
    let contents = output.stdout.replace(
        "server: https://127.0.0.1:6443",
        &format!("server: https://{ip_address}:6443"),
    );
    kubeconfig_file.write_all(contents.as_bytes())?;
}

#[throws(Error)]
fn verify_installation(node_name: &str) {
    progress!("Verifying installation");
    process!("kubectl wait --for=condition=ready node {node_name} --timeout=120s")
        .container_mode()
        .run()?;
}

#[throws(Error)]
fn report(node_name: &str) {
    kv!("nodes/{node_name}/initialized").update(true)?;
    info!("{node_name} has been successfully deployed");
}
