use std::{
    fmt::{self, Display, Formatter},
    time::Duration,
};

use clap::Parser;
use indexmap::IndexMap;

use crate::{
    prelude::*,
    process::{self, ProcessBuilder},
};

#[throws(anyhow::Error)]
pub fn get_attached_disks() -> Vec<DiskInfo> {
    match process!("uname").run()?.stdout.trim() {
        "Linux" => {
            let output = process!("lsblk -bOJ").run()?;
            serde_json::from_slice::<linux::LsblkOutput>(output.stdout.as_bytes())?.into()
        }
        "Darwin" => {
            let output = process!("diskutil list -plist external physical").run()?;
            plist::from_bytes::<macos::DiskutilOutput>(output.stdout.as_bytes())?.into()
        }
        os => bail!("Unsupported operating system: {os}"),
    }
}

#[throws(anyhow::Error)]
pub fn k8s_wait_on_pods(deployment_name: &str) {
    const JSON_PATH: &str = "'\
        {range .items[*]}\
            {.status.phase}{\"=\"}\
            {range .status.containerStatuses[*]}\
                {.ready}{\"-\"}\
            {end}{\",\"}\
        {end}'";
    const MAX_ATTEMPTS: usize = 30;

    let mut attempt = 0;
    loop {
        if attempt >= MAX_ATTEMPTS {
            bail!("Timing out waiting for pods to become ready")
        }

        let output = Kubectl
            .get()
            .pods()
            .not_selector("app.kubernetes.io/managed-by", "Helm")
            .selector("app.kubernetes.io/instance", deployment_name)
            .output(&format!("jsonpath={JSON_PATH}"))
            .run()?;

        let all_ready = output.stdout.split_terminator(',').all(|pod| {
            pod.split_once('=').map_or(false, |(phase, statuses)| {
                phase == "Running"
                    && statuses
                        .split_terminator('-')
                        .all(|status| status == "true")
            })
        });

        if all_ready {
            break;
        };

        spin_sleep::sleep(Duration::from_secs(10));

        attempt += 1;
    }
}

fn unnamed_if_empty<S: AsRef<str> + ?Sized>(name: &S) -> String {
    if name.as_ref().trim().is_empty() {
        "<unnamed>".to_owned()
    } else {
        format!(r#""{}""#, name.as_ref())
    }
}

#[derive(Parser)]
pub struct Defaults {
    /// Skip prompts for fields that have defaults
    ///
    /// This is equivalent to providing all defaultable flags without a value.
    #[clap(short, long)]
    defaults: bool,
}

#[derive(Debug, Clone)]
pub struct DiskInfo {
    pub id: String,
    pub part_type: String,
    pub name: String,
    pub size: usize,
    pub partitions: Vec<DiskPartitionInfo>,
}

#[derive(Debug, Clone)]
pub struct DiskPartitionInfo {
    pub id: String,
    pub size: usize,
    pub name: String,
}

impl DiskInfo {
    pub fn description(&self) -> String {
        let mut desc = format!("{}: ", self.id);
        desc += &unnamed_if_empty(&self.name);
        if !self.partitions.is_empty() {
            desc += &format!(
                " ({} partition{}: {})",
                self.partitions.len(),
                if self.partitions.len() == 1 { "" } else { "s" },
                self.partitions
                    .iter()
                    .map(|p| unnamed_if_empty(&p.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        desc + &format!(", {:.2} GB", self.size as f64 / 1e9)
    }
}

impl Display for DiskInfo {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        self.description().fmt(f)
    }
}

impl DiskPartitionInfo {
    fn description(&self) -> String {
        format!(
            "{}: {} ({:.2} GB)",
            self.id,
            unnamed_if_empty(&self.name),
            self.size as f64 / 1e9,
        )
    }
}

impl Display for DiskPartitionInfo {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        self.description().fmt(f)
    }
}

pub struct Kubectl;

impl Kubectl {
    pub fn get(self) -> KubectlGet {
        KubectlGet
    }
}

pub struct KubectlGet;

impl KubectlGet {
    pub fn pods<'a>(self) -> KubectlGetPods<'a> {
        KubectlGetPods::new()
    }
}

pub struct KubectlGetPods<'a> {
    selectors: IndexMap<&'a str, (&'static str, &'a str)>,
    output: Option<&'a str>,
}

impl<'a> KubectlGetPods<'a> {
    pub fn new() -> Self {
        Self {
            selectors: IndexMap::new(),
            output: None,
        }
    }

    pub fn selector(mut self, key: &'a str, value: &'a str) -> Self {
        self.selectors.insert(key, ("=", value));
        self
    }

    pub fn not_selector(mut self, key: &'a str, value: &'a str) -> Self {
        self.selectors.insert(key, ("!=", value));
        self
    }

    pub fn output(mut self, output: &'a str) -> Self {
        self.output.replace(output);
        self
    }

    #[throws(process::Error)]
    pub fn run(self) -> process::Output {
        ProcessBuilder::from(self).run()?
    }
}

impl From<KubectlGetPods<'_>> for ProcessBuilder {
    fn from(get_pods: KubectlGetPods) -> Self {
        let mut process = String::from("kubectl get pods");

        for (key, (constraint, value)) in get_pods.selectors {
            process += " --selector=";
            process += key;
            process += constraint;
            process += value;
        }

        if let Some(output) = get_pods.output {
            process += " --output=";
            process += output;
        }

        process!("{process}")
    }
}

pub struct Helm;

impl Helm {
    pub fn upgrade<'a>(self, name: &'a str, path: &'a str) -> HelmUpgrade<'a> {
        HelmUpgrade::new(name, path)
    }

    pub fn repo(self) -> HelmRepo {
        HelmRepo
    }
}

pub struct HelmRepo;

impl HelmRepo {
    pub fn update(self) -> HelmRepoUpdate {
        HelmRepoUpdate
    }
}

pub struct HelmRepoUpdate;

impl From<HelmRepoUpdate> for ProcessBuilder {
    fn from(_: HelmRepoUpdate) -> Self {
        process!("helm repo update")
    }
}

pub struct HelmUpgrade<'a> {
    name: &'a str,
    path: &'a str,
    install: bool,
    atomic: bool,
    timeout: Option<&'a str>,
    namespace: Option<&'a str>,
    create_namespace: bool,
    version: Option<&'a str>,
    settings: IndexMap<&'a str, &'a str>,
}

impl<'a> HelmUpgrade<'a> {
    fn new(name: &'a str, path: &'a str) -> Self {
        Self {
            name,
            path,
            install: true,
            atomic: true,
            timeout: Some("5m0s"),
            namespace: None,
            create_namespace: false,
            version: None,
            settings: IndexMap::new(),
        }
    }

    pub fn timeout(mut self, timeout: &'a str) -> Self {
        self.timeout.replace(timeout);
        self
    }

    pub fn namespace(mut self, namespace: &'a str) -> Self {
        self.namespace.replace(namespace);
        self
    }

    pub fn create_namespace(mut self) -> Self {
        self.create_namespace = true;
        self
    }

    pub fn version(mut self, version: &'a str) -> Self {
        self.version.replace(version);
        self
    }

    pub fn set(mut self, key: &'a str, value: &'a str) -> Self {
        self.settings.insert(key, value);
        self
    }
}

impl From<HelmUpgrade<'_>> for ProcessBuilder {
    fn from(upgrade: HelmUpgrade<'_>) -> Self {
        let mut process = format!(
            "helm upgrade {name} {path}",
            name = upgrade.name,
            path = upgrade.path,
        );

        if upgrade.install {
            process += " --install";
        }

        if upgrade.atomic {
            process += " --atomic";
        }

        if let Some(timeout) = upgrade.timeout {
            process += " --timeout=";
            process += timeout;
        }

        if let Some(namespace) = upgrade.namespace {
            process += " --namespace=";
            process += namespace;
        }

        if upgrade.create_namespace {
            process += " --create-namespace";
        }

        if let Some(version) = upgrade.version {
            process += " --version=";
            process += version;
        }

        for (key, value) in upgrade.settings {
            process += " --set=";
            process += key;
            process += "=";
            process += value;
        }

        process!("{process}")
    }
}

mod linux {
    use serde::{Deserialize, Deserializer};

    use super::*;

    #[throws(D::Error)]
    fn nullable_field<'de, D, T>(deserializer: D) -> T
    where
        D: Deserializer<'de>,
        T: Deserialize<'de> + Default,
    {
        let opt = Option::<T>::deserialize(deserializer)?;
        opt.unwrap_or_default()
    }

    #[derive(Deserialize)]
    pub struct LsblkOutput {
        blockdevices: Vec<LsblkDisk>,
    }

    #[derive(Deserialize)]
    struct LsblkDisk {
        name: String,
        #[serde(deserialize_with = "nullable_field")]
        fstype: String,
        kname: String,
        size: usize,
        #[serde(default = "Vec::new")]
        children: Vec<LsblkPartition>,
    }

    #[derive(Deserialize)]
    struct LsblkPartition {
        name: String,
        #[serde(deserialize_with = "nullable_field")]
        label: String,
        size: usize,
    }

    impl From<LsblkOutput> for Vec<DiskInfo> {
        fn from(output: LsblkOutput) -> Self {
            output
                .blockdevices
                .into_iter()
                .map(DiskInfo::from)
                .collect()
        }
    }

    impl From<LsblkDisk> for DiskInfo {
        fn from(disk: LsblkDisk) -> Self {
            Self {
                id: disk.name,
                name: disk.kname,
                size: disk.size,
                part_type: disk.fstype,
                partitions: disk.children.into_iter().map(Into::into).collect(),
            }
        }
    }

    impl From<LsblkPartition> for DiskPartitionInfo {
        fn from(partition: LsblkPartition) -> Self {
            Self {
                id: partition.name,
                name: partition.label,
                size: partition.size,
            }
        }
    }
}

mod macos {
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct DiskutilOutput {
        all_disks_and_partitions: Vec<DiskutilDisk>,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct DiskutilDisk {
        device_identifier: String,
        #[serde(default = "String::new")]
        volume_name: String,
        size: usize,
        content: String,
        #[serde(default = "Vec::new")]
        partitions: Vec<DiskutilPartition>,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct DiskutilPartition {
        device_identifier: String,
        #[serde(default = "String::new")]
        volume_name: String,
        size: usize,
    }

    impl From<DiskutilOutput> for Vec<DiskInfo> {
        fn from(output: DiskutilOutput) -> Self {
            output
                .all_disks_and_partitions
                .into_iter()
                .map(DiskInfo::from)
                .collect()
        }
    }

    impl From<DiskutilDisk> for DiskInfo {
        fn from(disk: DiskutilDisk) -> Self {
            Self {
                id: disk.device_identifier,
                name: disk.volume_name,
                size: disk.size,
                part_type: disk.content,
                partitions: disk.partitions.into_iter().map(Into::into).collect(),
            }
        }
    }

    impl From<DiskutilPartition> for DiskPartitionInfo {
        fn from(partition: DiskutilPartition) -> Self {
            Self {
                id: partition.device_identifier,
                name: partition.volume_name,
                size: partition.size,
            }
        }
    }
}
