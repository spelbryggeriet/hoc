use std::{
    borrow::Cow,
    cell::RefCell,
    ffi::OsStr,
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    net::IpAddr,
    path::{Path, PathBuf},
};

use colored::Colorize;
use hoc_core::{
    kv::{self, ReadStore, WriteStore},
    process::{self, ssh},
};
use hoc_log::{bail, choose, error, hidden_input, info, prompt, LogErr, Result};
use hoc_macros::{define_commands, doc_status, Procedure, ProcedureState};
use lazy_regex::regex;
use osshkeys::{keys::FingerprintHash, PublicKey, PublicParts};
use regex::Regex;
use serde::{Deserialize, Serialize};
use structopt::StructOpt;
use xz2::read::XzDecoder;
use zip::ZipArchive;

use crate::command::util::{cidr::Cidr, disk, os::OperatingSystem};

const VAULT_URL: &str = "https://releases.hashicorp.com/vault/1.10.3/vault_1.10.3_linux_arm64.zip";
const NOMAD_URL: &str = "https://releases.hashicorp.com/nomad/1.2.6/nomad_1.2.6_linux_arm64.zip";
const CONSUL_URL: &str =
    "https://releases.hashicorp.com/consul/1.11.4/consul_1.11.4_linux_arm64.zip";
const ENVOY_URL: &str =
    "https://archive.tetratelabs.io/envoy/download/v1.22.0/envoy-v1.22.0-linux-arm64.tar.xz";
const GO_URL: &str = "https://go.dev/dl/go1.18.2.linux-arm64.tar.gz";
const CFSSL_PKG_PATH: &str = "github.com/cloudflare/cfssl/cmd/cfssl";
const CFSSLJSON_PKG_PATH: &str = "github.com/cloudflare/cfssl/cmd/cfssljson";

const VAULT_VERSION: &str = "1.10.3";
const NOMAD_VERSION: &str = "1.2.6";
const CONSUL_VERSION: &str = "1.11.4";
const ENVOY_VERSION: &str = "1.22.0";
const GO_VERSION: &str = "1.18.2";
const CFSSL_VERSION: &str = "v1.6.1";

const LOCAL_DIR: &str = "/usr/local";
const BIN_DIR: &str = "/usr/local/bin";
const VAULT_TMP_DIR: &str = "/run/vault";
const NOMAD_TMP_DIR: &str = "/run/nomad";
const CONSUL_TMP_DIR: &str = "/run/consul";
const ENVOY_TMP_DIR: &str = "/run/envoy";
const GO_TMP_DIR: &str = "/run/go";

const VAULT_CERTS_DIR: &str = "/etc/vault.d/certs";
const VAULT_CA_PUB_FILENAME: &str = "vault-ca.pem";
const VAULT_CA_PRIV_FILENAME: &str = "vault-ca-key.pem";
const VAULT_SERVER_PUB_FILENAME: &str = "vault-cert.pem";
const VAULT_SERVER_PRIV_FILENAME: &str = "vault-key.pem";

const NOMAD_CERTS_DIR: &str = "/etc/nomad.d/certs";
const NOMAD_CA_PUB_FILENAME: &str = "nomad-ca.pem";
const NOMAD_CA_PRIV_FILENAME: &str = "nomad-ca-key.pem";
const NOMAD_SERVER_PUB_FILENAME: &str = "server.pem";
const NOMAD_SERVER_PRIV_FILENAME: &str = "server-key.pem";
const NOMAD_CLIENT_PUB_FILENAME: &str = "client.pem";
const NOMAD_CLIENT_PRIV_FILENAME: &str = "client-key.pem";
const NOMAD_CLI_PUB_FILENAME: &str = "cli.pem";
const NOMAD_CLI_PRIV_FILENAME: &str = "cli-key.pem";

const CONSUL_CERTS_DIR: &str = "/etc/consul.d/certs";
const CONSUL_CA_PUB_FILENAME: &str = "consul-agent-ca.pem";
const CONSUL_CA_PRIV_FILENAME: &str = "consul-agent-ca-key.pem";

#[derive(Procedure, StructOpt)]
#[procedure(dependencies(PrepareCluster(cluster=cluster)))]
pub struct DeployNode {
    #[procedure(attribute)]
    #[structopt(long)]
    cluster: String,

    #[procedure(attribute)]
    #[structopt(long)]
    node_name: String,

    #[structopt(long)]
    node_os: OperatingSystem,

    #[structopt(skip)]
    password: RefCell<Option<String>>,

    #[structopt(skip)]
    ssh_client: ssh::Client,
}

impl DeployNode {
    fn get_os_image_path(&self, registry: &impl ReadStore) -> Result<PathBuf> {
        Ok(registry
            .get(format!("$/images/{}", self.node_os))?
            .try_into()?)
    }

    fn get_address(&self, registry: &impl ReadStore) -> Result<IpAddr> {
        registry
            .get(format!(
                "clusters/{}/nodes/{}/network/address",
                self.cluster, self.node_name
            ))
            .and_then(String::try_from)?
            .parse()
            .log_err()
    }

    fn get_user(&self, registry: &impl ReadStore) -> Result<String> {
        Ok(registry
            .get(format!("clusters/{}/admin/username", self.cluster))?
            .try_into()?)
    }

    fn get_prefix_len(&self, registry: &impl ReadStore) -> Result<u32> {
        Ok(registry
            .get(format!("clusters/{}/network/prefix_len", self.cluster))?
            .try_into()?)
    }

    fn get_password(&self) -> String {
        if self.password.borrow().is_none() {
            let password = hidden_input!("[admin] Password").get();
            self.password.replace(Some(password));
        }

        self.password.borrow().clone().unwrap()
    }

    fn get_pub_key_path(&self, registry: &impl ReadStore) -> Result<PathBuf> {
        Ok(registry
            .get(format!("clusters/{}/admin/ssh/pub", self.cluster))?
            .try_into()?)
    }

    fn get_priv_key_path(&self, registry: &impl ReadStore) -> Result<PathBuf> {
        Ok(registry
            .get(format!("clusters/{}/admin/ssh/priv", self.cluster))?
            .try_into()?)
    }

    fn connect(
        &self,
        registry: &impl ReadStore,
        mut options: ssh::Options,
    ) -> Result<&ssh::Client> {
        if options.host.is_none() {
            options
                .host
                .replace(self.get_address(registry)?.to_string());
        }
        if options.user.is_none() {
            options.user.replace(self.get_user(registry)?);
        }

        if options.password.is_none() {
            options.password.replace(self.get_password());
        }

        if options.auth.is_none() {
            options.auth.replace(ssh::Authentication::Key {
                pub_key_path: self.get_pub_key_path(registry)?,
                priv_key_path: self.get_priv_key_path(registry)?,
            });
        }

        self.ssh_client.update(options);
        self.ssh_client.connect()?;

        Ok(&self.ssh_client)
    }
}

#[derive(ProcedureState, Serialize, Deserialize)]
pub enum DeployNodeState {
    #[state(transient)]
    DownloadImage,
    DecompressZipArchive,
    DecompressXzFile,

    AssignIpAddress,
    FlashImage,
    #[state(transient)]
    MountSdCard,
    #[state(transient)]
    ModifyRaspberryPiOsImage {
        disk_partition_id: String,
    },
    #[state(transient)]
    ModifyUbuntuImage {
        disk_partition_id: String,
    },
    UnmountSdCard {
        disk_partition_id: String,
    },
    AwaitNode,

    AddNewUser,
    AssignSudoPrivileges,
    DeletePiUser,
    SetUpSshAccess,

    ChangePassword,
    MountStorage,

    InstallDependencies,
    InitializeConsul,
    SetUpConsulAcl,
    InitializeVault,
    InitializeNomad,
    #[state(finish)]
    SetUpNomadAcl,
}

#[doc_status]
impl Run for DeployNodeState {
    #[define_commands(bsd_file = "file")]
    fn download_image(proc: &mut DeployNode, registry: &impl WriteStore) -> Result<Self> {
        let os = proc.node_os;

        let file_ref = match registry.create_file(format!("$/images/{os}")) {
            /// Download image
            Ok(file_ref) => {
                let image_url = os.image_url();
                info!("URL: {}", image_url);

                let mut file = File::options().write(true).open(file_ref.path())?;

                reqwest::blocking::get(image_url)
                    .log_err()?
                    .copy_to(&mut file)
                    .log_err()?;

                file_ref
            }

            Err(kv::Error::KeyAlreadyExists(_)) => {
                info!("Using cached file");
                return Ok(FlashImage);
            }

            Err(error) => return Err(error.into()),
        };

        /// Determine file type
        let state = {
            let output = bsd_file!("{}", file_ref.path().to_string_lossy())
                .run()?
                .1
                .to_lowercase();
            if output.contains("zip archive") {
                info!("Zip archive file type detected");
                DecompressZipArchive
            } else if output.contains("xz compressed data") {
                info!("XZ compressed data file type detected");
                DecompressXzFile
            } else {
                error!("Unsupported file type")?.into()
            }
        };

        Ok(state)
    }

    fn decompress_zip_archive(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        /// Read ZIP archive
        let (image_data, mut image_file) = {
            let archive_path = proc.get_os_image_path(registry)?;
            let file = File::options().read(true).write(true).open(&archive_path)?;

            let mut archive = ZipArchive::new(&file).log_err()?;

            let mut buf = None;
            let archive_len = archive.len();
            for i in 0..archive_len {
                let mut archive_file = archive
                    .by_index(i)
                    .log_context("Failed to lookup image in ZIP archive")?;

                if archive_file.is_file() && archive_file.name().ends_with(".img") {
                    info!("Found image at index {} among {} items.", i, archive_len);

                    let mut data = Vec::new();
                    {
                        //! Decompress image

                        archive_file
                            .read_to_end(&mut data)
                            .log_context("Failed to read image in ZIP archive")?;
                        buf.replace(data);
                    }
                    break;
                }
            }

            if let Some(data) = buf {
                (data, file)
            } else {
                bail!("Image not found within ZIP archive");
            }
        };

        /// Save decompressed image to file
        {
            image_file.seek(SeekFrom::Start(0))?;
            image_file.set_len(0)?;
            image_file.write_all(&image_data)?;
        }

        Ok(FlashImage)
    }

    fn decompress_xz_file(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        /// Read XZ file
        let (image_data, mut image_file) = {
            let file_path = proc.get_os_image_path(registry)?;
            let file = File::options().read(true).write(true).open(&file_path)?;

            let mut decompressor = XzDecoder::new(&file);

            let mut buf = Vec::new();

            /// Decompress image
            decompressor
                .read_to_end(&mut buf)
                .log_context("Failed to read image in XZ file")?;

            (buf, file)
        };

        /// Save decompressed image to file
        {
            image_file.seek(SeekFrom::Start(0))?;
            image_file.set_len(0)?;
            image_file.write_all(&image_data)?;
        }

        Ok(AssignIpAddress)
    }

    fn assign_ip_address(proc: &mut DeployNode, registry: &impl WriteStore) -> Result<Self> {
        let cluster = &proc.cluster;
        let node = &proc.node_name;
        let start_address: IpAddr = registry
            .get(format!("clusters/{cluster}/network/start_address"))
            .and_then(String::try_from)?
            .parse()
            .log_err()?;
        let used_addresses: Vec<IpAddr> =
            match registry.get(format!("clusters/{cluster}/nodes/*/network/address")) {
                Ok(item) => Vec::<String>::try_from(item)?,
                Err(kv::Error::KeyDoesNotExist(_)) => Vec::new(),
                Err(err) => return Err(err.into()),
            }
            .into_iter()
            .map(|s| s.parse().log_err())
            .collect::<Result<_>>()?;
        let prefix_len = proc.get_prefix_len(registry)?;

        let addresses = Cidr {
            ip_addr: start_address,
            prefix_len,
        };
        for step in 0.. {
            let next_address = addresses.step(step).log_err()?;
            if !used_addresses.contains(&next_address) {
                registry.put(
                    format!("clusters/{cluster}/nodes/{node}/network/address"),
                    next_address.to_string(),
                )?;

                info!("Address {next_address} asigned.");

                break;
            }
        }
        let inc = Self::numeral(used_addresses.len() as u64 + 1);
        registry.put(
            format!("clusters/{cluster}/nodes/{node}/nomad/name"),
            format!("node-{inc}"),
        )?;

        Ok(FlashImage)
    }

    #[define_commands(dd, diskutil)]
    fn flash_image(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        let os = proc.node_os;

        /// Choose SD card
        let disk = {
            let mut disks: Vec<_> = disk::get_attached_disks(None)
                .log_context("Failed to get attached disks")?
                .collect();

            let index = if disks.len() == 1 {
                0
            } else {
                choose!("Which disk is your SD card?").items(&disks).get()?
            };

            disks.remove(index)
        };

        /// Unmount SD card
        diskutil!("unmountDisk {}", disk.id).run()?;

        /// Flash SD card
        {
            let image_path = proc.get_os_image_path(registry)?;

            prompt!(
                "Do you want to flash target disk '{}' with operating system '{os}'?",
                disk.description(),
            )
            .get()?;

            dd!(
                "bs=1m if={} of=/dev/r{}",
                image_path.to_string_lossy(),
                disk.id,
            )
            .sudo()
            .run()?;
        }

        Ok(MountSdCard)
    }

    #[define_commands(diskutil)]
    fn mount_sd_card(proc: &mut DeployNode, _registry: &impl ReadStore) -> Result<Self> {
        let os = proc.node_os;

        let boot_partition_name = match os {
            OperatingSystem::RaspberryPiOs { .. } => "boot",
            OperatingSystem::Ubuntu { .. } => "system-boot",
        };

        /// Mount boot partition
        let disk_partition_id = {
            let mut partitions: Vec<_> = disk::get_attached_disk_partitions(None)
                .log_context("Failed to get attached disks")?
                .filter(|p| p.name == boot_partition_name)
                .collect();

            let index = if partitions.len() == 1 {
                0
            } else {
                choose!("Which refers to the boot partition of the disk?")
                    .items(&partitions)
                    .get()?
            };

            let disk_partition = partitions.remove(index);

            prompt!(
                "Do you want to mount disk partition '{}'?",
                disk_partition.description(),
            )
            .get()?;

            diskutil!("mount {}", disk_partition.id).run()?;

            disk_partition.id
        };

        let state = match os {
            OperatingSystem::RaspberryPiOs { .. } => ModifyRaspberryPiOsImage { disk_partition_id },
            OperatingSystem::Ubuntu { .. } => ModifyUbuntuImage { disk_partition_id },
        };

        Ok(state)
    }

    fn modify_raspberry_pi_os_image(
        _proc: &mut DeployNode,
        _registry: &impl ReadStore,
        disk_partition_id: String,
    ) -> Result<Self> {
        let mount_dir = disk::find_mount_dir(&disk_partition_id)?;

        /// Configure image
        /// Create SSH file
        File::create(mount_dir.join("ssh"))?;

        Ok(UnmountSdCard { disk_partition_id })
    }

    fn modify_ubuntu_image(
        proc: &mut DeployNode,
        registry: &impl WriteStore,
        disk_partition_id: String,
    ) -> Result<Self> {
        let cluster = &proc.cluster;
        let node = &proc.node_name;
        let username = proc.get_user(registry)?;
        let pub_key_path = proc.get_pub_key_path(registry)?;
        let address = proc.get_address(registry)?;
        let prefix_len = proc.get_prefix_len(registry)?;
        let gateway: IpAddr = registry
            .get(format!("clusters/{cluster}/network/gateway"))
            .and_then(String::try_from)?
            .parse()
            .log_err()?;
        let nomad_name: String = registry
            .get(format!("clusters/{cluster}/nodes/{node}/nomad/name"))?
            .try_into()?;

        /// Read SSH keypair
        let pub_key = {
            let pub_key = fs::read_to_string(pub_key_path)?;
            info!(
                "SSH public key fingerprint randomart:\n{}",
                PublicKey::from_keystr(&pub_key)
                    .log_err()?
                    .fingerprint_randomart(FingerprintHash::SHA256)
                    .log_err()?
            );

            pub_key
        };

        // Get all node IP addresses in the cluster.
        let cluster_addresses = Vec::<String>::try_from(
            registry.get(format!("clusters/{cluster}/nodes/*/network/address"))?,
        )?
        .join(r#"", ""#);

        let mount_dir = disk::find_mount_dir(&disk_partition_id)?;

        /// Prepare image initialization
        {
            let data_map: serde_yaml::Value = serde_yaml::from_str(&format!(
                include_str!("../../config/user-data"),
                admin_username = username,
                cluster = cluster,
                cluster_addresses = cluster_addresses,
                hostname = proc.node_name,
                ip_address = address,
                nomad_name = nomad_name,
                ssh_pub_key = pub_key,
            ))
            .log_context("invalid user-data format")?;

            let data = serde_yaml::to_string(&data_map).log_err()?;
            let data = "#cloud-config".to_string() + data.strip_prefix("---").unwrap_or(&data);
            info!(
                "Updating {} with the following configuration:\n{data}",
                "/user-data".blue(),
            );

            let user_data_path = mount_dir.join("user-data");
            fs::write(&user_data_path, &data)?;

            let data_map: serde_yaml::Value = serde_yaml::from_str(&format!(
                include_str!("../../config/network-config"),
                address = Cidr {
                    ip_addr: address.clone(),
                    prefix_len: prefix_len,
                },
                gateway = gateway,
                gateway_ip_version = if gateway.is_ipv4() {
                    4
                } else if gateway.is_ipv6() {
                    6
                } else {
                    error!("Unspecified gateway IP version")?.into()
                },
            ))
            .log_context("invalid network-config format")?;

            let data = serde_yaml::to_string(&data_map).log_err()?;
            let data = data
                .strip_prefix("---\n")
                .map(ToString::to_string)
                .unwrap_or(data);
            info!(
                "Updating {} with the following configuration:\n{}",
                "/network-config".blue(),
                data
            );

            let network_config_path = mount_dir.join("network-config");
            fs::write(&network_config_path, &data)?;
        }

        Ok(UnmountSdCard { disk_partition_id })
    }

    #[define_commands(diskutil, sync)]
    fn unmount_sd_card(
        _proc: &mut DeployNode,
        _registry: &impl ReadStore,
        disk_partition_id: String,
    ) -> Result<Self> {
        /// Sync image disk writes
        sync!().run()?;

        /// Unmount image disk
        diskutil!("unmount {disk_partition_id}").run()?;

        Ok(AwaitNode)
    }

    #[define_commands(cloud_init = "cloud-init", ping)]
    fn await_node(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        let os = proc.node_os;
        let address = proc.get_address(registry)?;

        info!(
            "The SD card has now been prepared. Take out the SD card and insert it into the \
            node hardware. Then, plug it in to the network via ethernet and power it on."
        );
        prompt!("Have you prepared the node hardware?").get()?;

        ping!("-o -t 300 -i 5 {address}").run()?;

        let state = match os {
            OperatingSystem::RaspberryPiOs { .. } => AddNewUser,
            OperatingSystem::Ubuntu { .. } => {
                let client = proc.connect(registry, ssh::Options::default())?;
                cloud_init!("status --wait").ssh(&client).run()?;
                ChangePassword
            }
        };

        Ok(state)
    }

    #[define_commands(adduser)]
    fn add_new_user(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        let username: String = proc.get_user(registry)?;
        let password = proc.get_password();
        let client = proc.connect(
            registry,
            ssh::Options::default().user("pi").password("raspberry"),
        )?;

        // Add the new user.
        adduser!("{username}")
            .stdin_lines([&*password, &*password])
            .sudo()
            .ssh(&client)
            .run()?;

        Ok(AssignSudoPrivileges)
    }

    #[define_commands(chmod, tee, usermod)]
    fn assign_sudo_privileges(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        let username: String = proc.get_user(registry)?;
        let client = proc.connect(
            registry,
            ssh::Options::default().user("pi").password("raspberry"),
        )?;
        let common_set = process::Settings::default().sudo().ssh(&client);

        // Assign the user the relevant groups.
        usermod!(
            "-a -G adm,dialout,cdrom,sudo,audio,video,plugdev,games,users,input,netdev,gpio,i2c,spi \
                {username}",
        )
        .run_with(&common_set)?;

        // Create sudo file for the user.
        let sudo_file = format!("/etc/sudoers.d/010_{username}");
        tee!("{sudo_file}")
            .settings(&common_set)
            .stdin_line(&format!("{username} ALL=(ALL) PASSWD: ALL"))
            .hide_output()
            .run()?;
        chmod!("440 {sudo_file}").run_with(&common_set)?;

        Ok(DeletePiUser)
    }

    #[define_commands(deluser, pkill)]
    fn delete_pi_user(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        let password = proc.get_password();
        let client = proc.connect(registry, ssh::Options::default().password_auth())?;
        let common_set = process::Settings::default()
            .sudo_password(&*password)
            .ssh(&client);

        // Kill all processes owned by the `pi` user.
        pkill!("-u pi")
            .settings(&common_set)
            .success_codes([0, 1])
            .run()?;

        // Delete the default `pi` user.
        deluser!("--remove-home pi").run_with(&common_set)?;

        Ok(SetUpSshAccess)
    }

    #[define_commands(bsd_test = "test", chmod, mkdir, sed, sshd, systemctl, tee)]
    fn set_up_ssh_access(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        let username = proc.get_user(registry)?;
        let password = proc.get_password();
        let pub_path = proc.get_pub_key_path(registry)?;
        let client = proc.connect(registry, ssh::Options::default().password_auth())?;
        let common_set = process::Settings::default().ssh(&client);
        let sudo_set = common_set.clone().sudo_password(&*password);

        /// Read SSH keypair
        let pub_key = {
            let pub_key = fs::read_to_string(&pub_path)?;
            info!(
                "SSH public key fingerprint randomart:\n{}",
                PublicKey::from_keystr(&pub_key)
                    .log_err()?
                    .fingerprint_randomart(FingerprintHash::SHA256)
                    .log_err()?
            );

            pub_key
        };

        /// Send SSH public key
        {
            // Create the `.ssh` directory.
            mkdir!("-p -m 700 /home/{username}/.ssh").run_with(&common_set)?;

            let authorized_keys_path = format!("/home/{username}/.ssh/authorized_keys");

            // Check if the authorized keys file exists.
            let (status_code, _) = bsd_test!("-s {authorized_keys_path}")
                .settings(&common_set)
                .success_codes([0, 1])
                .run()?;
            if status_code == 1 {
                // Create the authorized keys file.
                tee!("{authorized_keys_path}")
                    .settings(&common_set)
                    .stdin_line(&username)
                    .run()?;
                chmod!("644 {authorized_keys_path}").run_with(&common_set)?;
            }

            // Copy the public key to the authorized keys file.
            let key = pub_key.replace("/", r"\/");
            let auth_keys_sed = format!(
                "0,/{username}$/{{h;s/^.*{username}$/{key}/}};${{x;/^$/{{s//{key}/;H}};x}}"
            );
            sed!("-i {auth_keys_sed} {authorized_keys_path}",)
                .settings(&common_set)
                .secret(&key)
                .run()?;
        }

        /// Initialize SSH server
        {
            let sshd_config_path = "/etc/ssh/sshd_config";

            // Set `PasswordAuthentication` to `no`.
            let key = "PasswordAuthentication";
            let pass_auth_sed =
                format!("0,/{key}/{{h;s/^.*{key}.*$/{key} no/}};${{x;/^$/{{s//{key} no/;H}};x}}");
            sed!("-i {pass_auth_sed} {sshd_config_path}",)
                .settings(&common_set)
                .run()?;

            // Verify sshd config and restart the SSH server.
            sshd!("-t").run_with(&sudo_set)?;
            systemctl!("restart ssh").run_with(&sudo_set)?;

            // Verify again after SSH server restart.
            let client = proc.connect(registry, ssh::Options::default())?;

            sshd!("-t").settings(&sudo_set).ssh(&client).run()?;
        }

        Ok(InstallDependencies)
    }

    #[define_commands(chpasswd)]
    fn change_password(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        let username = proc.get_user(registry)?;
        let password = proc.get_password();
        let client = proc.connect(registry, ssh::Options::default())?;

        chpasswd!()
            .sudo_password("temporary_password")
            .stdin_line(format!("{username}:{password}"))
            .ssh(&client)
            .run()?;

        Ok(MountStorage)
    }

    #[define_commands(blkid, sed, cat)]
    fn mount_storage(proc: &mut DeployNode, registry: &impl WriteStore) -> Result<Self> {
        let os = proc.node_os;
        let password = proc.get_password();
        let client = proc.connect(registry, ssh::Options::default())?;
        let common_set = process::Settings::default().ssh(&client);
        let sudo_set = common_set.clone().sudo_password(&*password);

        let boot_partition_name = match os {
            OperatingSystem::RaspberryPiOs { .. } => "boot",
            OperatingSystem::Ubuntu { .. } => "system-boot",
        };

        let mut disks: Vec<_> = disk::get_attached_disks(Some(client))?
            .filter(|disk| {
                disk.partitions
                    .iter()
                    .all(|part| part.name != boot_partition_name)
            })
            .flat_map(|disk| disk.partitions)
            .collect();

        if disks.is_empty() {
            return Ok(InstallDependencies);
        }

        let selection = choose!("Which storage do you want to mount?")
            .items(&disks)
            .get()?;
        let id = disks.remove(selection).id;

        let (_, output) = blkid!("/dev/{id}").run_with(&common_set)?;
        let uuid = regex!(r#"^.*UUID="([^"]*)".*$"#)
            .captures(&output)
            .unwrap()
            .get(1)
            .unwrap()
            .as_str();

        let fstab_line = format!("UUID={uuid} /media auto nosuid,nodev,nofail 0 0");
        let fstab_line = fstab_line.replace("/", r"\/");
        let fstab_sed = format!("/^UUID={uuid}/{{h;s/^UUID={uuid}.*$/{fstab_line}/}};${{x;/^$/{{s//{fstab_line}/;H}};x}}");
        sed!("-i {fstab_sed} /etc/fstab").run_with(&sudo_set)?;

        cat!("/etc/fstab").run_with(&common_set)?;

        Ok(InstallDependencies)
    }

    #[define_commands(go, mkdir, mv, rm, tar, unzip, wget)]
    fn install_dependencies(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        let password = proc.get_password();
        let client = proc.connect(registry, ssh::Options::default())?;
        let common_set = process::Settings::default().ssh(&client);
        let sudo_set = common_set.clone().sudo_password(&*password);

        let consul_path = format!("{CONSUL_TMP_DIR}/{CONSUL_VERSION}.zip");
        let vault_path = format!("{VAULT_TMP_DIR}/{VAULT_VERSION}.zip");
        let nomad_path = format!("{NOMAD_TMP_DIR}/{NOMAD_VERSION}.zip");
        let envoy_path = format!("{ENVOY_TMP_DIR}/{ENVOY_VERSION}.tar.xz");
        let go_path = format!("{GO_TMP_DIR}/{GO_VERSION}.tar.xz");

        /// Consul
        {
            mkdir!("{CONSUL_TMP_DIR}").run_with(&sudo_set)?;
            wget!("--no-verbose {CONSUL_URL} -O {consul_path}").run_with(&sudo_set)?;
            unzip!("-o {consul_path} -d {BIN_DIR}").run_with(&sudo_set)?;
            rm!("-fr {CONSUL_TMP_DIR}").run_with(&sudo_set)?;
        }

        /// Vault
        {
            mkdir!("{VAULT_TMP_DIR}").run_with(&sudo_set)?;
            wget!("--no-verbose {VAULT_URL} -O {vault_path}").run_with(&sudo_set)?;
            unzip!("-o {vault_path} -d {BIN_DIR}").run_with(&sudo_set)?;
            rm!("-fr {VAULT_TMP_DIR}").run_with(&sudo_set)?;
        }

        /// Nomad
        {
            mkdir!("{NOMAD_TMP_DIR}").run_with(&sudo_set)?;
            wget!("--no-verbose {NOMAD_URL} -O {nomad_path}").run_with(&sudo_set)?;
            unzip!("-o {nomad_path} -d {BIN_DIR}").run_with(&sudo_set)?;
            rm!("-fr {NOMAD_TMP_DIR}").run_with(&sudo_set)?;
        }

        /// Envoy
        {
            mkdir!("{ENVOY_TMP_DIR}").run_with(&sudo_set)?;
            wget!("--no-verbose {ENVOY_URL} -O {envoy_path}").run_with(&sudo_set)?;
            tar!("-xJf {envoy_path} --strip-components 2 -C {BIN_DIR}").run_with(&sudo_set)?;
            rm!("-fr {ENVOY_TMP_DIR}").run_with(&sudo_set)?;
        }

        /// Go
        {
            mkdir!("{GO_TMP_DIR}").run_with(&sudo_set)?;
            wget!("--no-verbose {GO_URL} -O {go_path}").run_with(&sudo_set)?;
            tar!("-xzf {go_path} -C {LOCAL_DIR}").run_with(&sudo_set)?;
            rm!("-fr {GO_TMP_DIR}").run_with(&sudo_set)?;
        }

        /// cfssl
        {
            go!("install {CFSSL_PKG_PATH}@{CFSSL_VERSION}").run_with(&common_set)?;
            go!("install {CFSSLJSON_PKG_PATH}@{CFSSL_VERSION}").run_with(&common_set)?;
            mv!("go/bin/cfssl go/bin/cfssljson {BIN_DIR}").run_with(&sudo_set)?;
        }

        Ok(InitializeConsul)
    }

    #[define_commands(chown, complete, consul, mkdir, mv, rm, sed, systemctl)]
    fn initialize_consul(proc: &mut DeployNode, registry: &impl WriteStore) -> Result<Self> {
        let cluster = &proc.cluster;
        let password = proc.get_password();
        let client = proc.connect(registry, ssh::Options::default())?;
        let common_set = process::Settings::default().ssh(&client);
        let sudo_set = common_set.clone().sudo_password(&*password);
        let consul_set = sudo_set.clone().sudo_user("consul");

        // Set up command autocomplete.
        consul!("-autocomplete-install")
            .settings(&consul_set)
            .success_codes([0, 1])
            .run()?;
        complete!("-C {BIN_DIR}/consul consul").run_with(&common_set)?;

        // Generate common cluster key.
        let registry_key = format!("$/clusters/{cluster}/consul/key");
        let encrypt_key: String = match registry.get(&registry_key).and_then(String::try_from) {
            Ok(key) => key,
            Err(kv::Error::KeyDoesNotExist(_)) => {
                let (_, key) = consul!("keygen")
                    .settings(&consul_set)
                    .hide_stdout()
                    .run()?;
                registry.put(&registry_key, key.clone())?;
                key
            }
            Err(err) => return Err(err.into()),
        };

        mkdir!("-p {CONSUL_CERTS_DIR}").run_with(&sudo_set)?;

        // Create or distribute certificate authority certificate.
        let ca_pub_key = format!("$/clusters/{cluster}/consul/certs/ca_pub");
        let ca_priv_key = format!("$/clusters/{cluster}/consul/certs/ca_priv");
        let ca_pub_path = format!("{CONSUL_CERTS_DIR}/{CONSUL_CA_PUB_FILENAME}");
        let ca_priv_path = format!("{CONSUL_CERTS_DIR}/{CONSUL_CA_PRIV_FILENAME}");
        match registry.get(&ca_pub_key).and_then(kv::FileRef::try_from) {
            Ok(ca_pub_file_ref) => {
                Self::upload_certificates(
                    client,
                    &*password,
                    ca_pub_file_ref.path(),
                    &ca_pub_path,
                    registry
                        .get(ca_priv_key)
                        .and_then(kv::FileRef::try_from)?
                        .path(),
                    &ca_priv_path,
                )?;
            }
            Err(kv::Error::KeyDoesNotExist(_)) => {
                // Create CA certificate.
                consul!("tls ca create").run_with(&sudo_set)?;
                mv!("{CONSUL_CA_PUB_FILENAME} {CONSUL_CA_PRIV_FILENAME} {CONSUL_CERTS_DIR}")
                    .run_with(&sudo_set)?;

                // Download certificate and key.
                Self::download_certificates(
                    registry,
                    client,
                    &*password,
                    &ca_pub_path,
                    &ca_priv_path,
                    &ca_pub_key,
                    &ca_priv_key,
                )?;
            }
            Err(err) => return Err(err.into()),
        }
        chown!("-R consul:consul {CONSUL_CERTS_DIR}").run_with(&sudo_set)?;

        // Create server certificates.
        let cert_filename = format!("{cluster}-server-consul-0.pem");
        let key_filename = format!("{cluster}-server-consul-0-key.pem");
        consul!("tls cert create -server -dc={cluster} -domain=consul -ca={ca_pub_path} -key={ca_priv_path}",
        )
        .run_with(&sudo_set)?;
        mv!("{cert_filename} {key_filename} {CONSUL_CERTS_DIR}").run_with(&sudo_set)?;
        chown!("-R consul:consul {CONSUL_CERTS_DIR}").run_with(&sudo_set)?;

        // Remove CA key.
        rm!("{ca_priv_path}").run_with(&sudo_set)?;

        // Update Consul configuration file
        let encrypt_sed = format!(
            r#"s/^(encrypt = ")temporary_key(")$/\1{}\2/"#,
            encrypt_key.replace("/", r"\/")
        );
        let consul_config_path = "/etc/consul.d/consul.hcl";
        sed!("-ri {encrypt_sed} {consul_config_path}").run_with(&sudo_set)?;

        // Validate Consul configuration.
        consul!("validate /etc/consul.d/").run_with(&consul_set)?;

        // Start Consul service.
        systemctl!("enable consul").run_with(&sudo_set)?;
        systemctl!("start consul").run_with(&sudo_set)?;

        Ok(SetUpConsulAcl)
    }

    #[define_commands(consul, rm, tee)]
    fn set_up_consul_acl(proc: &mut DeployNode, registry: &impl WriteStore) -> Result<Self> {
        let cluster = &proc.cluster;

        let client = proc.connect(registry, ssh::Options::default())?;
        let mut common_set = process::Settings::default().ssh(&client);
        let mut consul_set = process::Settings::default().ssh(&client);

        // Set up ACL.
        let mgmt_token_key = format!("$/clusters/{cluster}/consul/tokens/management");
        let node_token_key = format!("$/clusters/{cluster}/consul/tokens/node");
        let (mgmt_token, node_token) = match registry
            .get(&mgmt_token_key)
            .and_then(String::try_from)
        {
            Ok(token) => (
                token,
                registry.get(node_token_key).and_then(String::try_from)?,
            ),
            Err(kv::Error::KeyDoesNotExist(_)) => {
                // Bootstrap ACL.
                let (_, output) = consul!("acl bootstrap")
                    .settings(&consul_set)
                    .hide_stdout()
                    .run()?;
                let mgmt_token = Self::get_id("SecretID", &output).to_string();

                common_set = common_set.secret(mgmt_token.clone());
                consul_set = consul_set.secret(mgmt_token.clone());

                // Create ACL policy.
                tee!("node-policy.hcl")
                    .settings(&common_set)
                    .stdin_lines(include_str!("../../config/consul/node-policy.hcl").lines())
                    .run()?;

                consul!("acl policy create -token={mgmt_token} -name=node-policy -rules=@node-policy.hcl")
                    .run_with(&consul_set)?;

                rm!("node-policy.hcl").run_with(&common_set)?;

                // Create ACL token.
                let (_, output) = consul!("acl token create -token={mgmt_token} -description='node token' -policy-name=node-policy"
                )
                .settings(&consul_set)
                .hide_stdout()
                .run()?;
                let node_token = Self::get_id("SecretID", &output).to_string();

                consul_set = consul_set.secret(node_token.clone());

                registry.put(mgmt_token_key, mgmt_token.clone())?;
                registry.put(node_token_key, node_token.clone())?;

                // Create Vault policy.
                tee!("vault-service-policy.hcl")
                    .settings(&common_set)
                    .stdin_lines(
                        include_str!("../../config/consul/vault-service-policy.hcl").lines(),
                    )
                    .run()?;

                consul!(
                    "acl policy create -token={mgmt_token} -name=vault-service \
                        -rules=@vault-service-policy.hcl"
                )
                .run_with(&consul_set)?;

                rm!("vault-service-policy.hcl").run_with(&common_set)?;

                // Create Vault token.
                let (_, output) = consul!(
                    "acl token create -token={mgmt_token} -description='Vault Service Token' \
                        -policy-name=vault-service"
                )
                .settings(&consul_set)
                .hide_stdout()
                .run()?;
                let vault_token = Self::get_id("SecretID", &output).to_string();

                consul_set = consul_set.secret(vault_token.clone());

                let vault_token_key = format!("$/clusters/{cluster}/consul/tokens/vault");
                registry.put(vault_token_key, vault_token.clone())?;

                (mgmt_token, node_token)
            }
            Err(err) => return Err(err.into()),
        };

        // Assign ACL token to node.
        consul!("acl set-agent-token -token={mgmt_token} agent {node_token}")
            .run_with(&consul_set)?;

        Ok(InitializeVault)
    }

    #[define_commands(
        cat,
        chown,
        complete,
        cp,
        mkdir,
        mv,
        openssl,
        rm,
        sed,
        systemctl,
        tee,
        update_ca_certificates = "update-ca-certificates",
        vault
    )]
    fn initialize_vault(proc: &mut DeployNode, registry: &impl WriteStore) -> Result<Self> {
        let cluster = &proc.cluster;
        let node_name = &proc.node_name;
        let password = proc.get_password();
        let client = proc.connect(registry, ssh::Options::default())?;
        let common_set = process::Settings::default().ssh(&client);
        let sudo_set = common_set.clone().sudo_password(&*password);
        let vault_set = sudo_set.clone().sudo_user("vault");

        /// Set up command autocomplete
        {
            vault!("-autocomplete-install")
                .settings(&common_set)
                .success_codes([0, 1])
                .run()?;
            complete!("-C {BIN_DIR}/vault vault").run_with(&common_set)?;
        }

        mkdir!("-p {VAULT_CERTS_DIR}").run_with(&sudo_set)?;

        // Create or distribute certificate authority certificate.
        let ca_pub_key = format!("$/clusters/{cluster}/vault/certs/ca_pub");
        let ca_priv_key = format!("$/clusters/{cluster}/vault/certs/ca_priv");
        match registry.get(&ca_pub_key).and_then(kv::FileRef::try_from) {
            Ok(ca_pub_file_ref) => {
                Self::upload_certificates(
                    client,
                    &*password,
                    ca_pub_file_ref.path(),
                    &VAULT_CA_PUB_FILENAME,
                    registry
                        .get(ca_priv_key)
                        .and_then(kv::FileRef::try_from)?
                        .path(),
                    &VAULT_CA_PRIV_FILENAME,
                )?;
            }
            Err(kv::Error::KeyDoesNotExist(_)) => {
                // Create CA certificate.
                openssl!("ecparam -genkey -name prime256v1 -noout -out {VAULT_CA_PRIV_FILENAME}")
                    .settings(&common_set)
                    .hide_stdout()
                    .run()?;
                openssl!(
                    "req -x509 -new -nodes -key {VAULT_CA_PRIV_FILENAME} -sha256 -days 1825 \
                    -subj '/C=US/ST=CA/L=San Francisco/CN=example.net' \
                    -addext keyUsage=critical,keyCertSign,cRLSign \
                    -out {VAULT_CA_PUB_FILENAME}"
                )
                .run_with(&common_set)?;

                // Download certificate and key.
                Self::download_certificates(
                    registry,
                    client,
                    &*password,
                    VAULT_CA_PUB_FILENAME,
                    VAULT_CA_PRIV_FILENAME,
                    &ca_pub_key,
                    &ca_priv_key,
                )?;
            }
            Err(err) => return Err(err.into()),
        }

        // Install public CA file.
        cp!(
            "{VAULT_CA_PUB_FILENAME} /usr/local/share/ca-certificates/{}",
            VAULT_CA_PUB_FILENAME.replace("pem", "crt")
        )
        .run_with(&sudo_set)?;
        update_ca_certificates!().run_with(&sudo_set)?;

        // Format certificate request extension file.
        tee!("cert.conf")
            .settings(&common_set)
            .stdin_lines(
                format!(
                    include_str!("../../config/vault/cert.conf"),
                    common_name = node_name,
                )
                .lines(),
            )
            .run()?;

        // Create server certificates.
        openssl!("ecparam -genkey -name prime256v1 -noout -out {VAULT_SERVER_PRIV_FILENAME}")
            .settings(&common_set)
            .hide_stdout()
            .run()?;
        openssl!("req -new -key {VAULT_SERVER_PRIV_FILENAME} -subj '/' -out server.csr")
            .run_with(&common_set)?;
        openssl!(
            "x509 -req -in server.csr -CA {VAULT_CA_PUB_FILENAME} -CAkey {VAULT_CA_PRIV_FILENAME} \
                -CAcreateserial -sha256 -days 1825 -extfile cert.conf \
                -out {VAULT_SERVER_PUB_FILENAME}"
        )
        .run_with(&common_set)?;

        // Create certificate chain.
        let server_pub_path = format!("{VAULT_CERTS_DIR}/{VAULT_SERVER_PUB_FILENAME}");
        mv!("{VAULT_SERVER_PRIV_FILENAME} {VAULT_CERTS_DIR}").run_with(&sudo_set)?;
        cat!("{VAULT_SERVER_PUB_FILENAME} {VAULT_CA_PUB_FILENAME}")
            .settings(&common_set)
            .stdout(&format!("{VAULT_SERVER_PUB_FILENAME}.chain"))
            .run()?;
        mv!("{VAULT_SERVER_PUB_FILENAME}.chain {server_pub_path}").run_with(&sudo_set)?;
        chown!("-R vault:vault {VAULT_CERTS_DIR}").run_with(&sudo_set)?;

        // Remove extraneous files.
        rm!(
            "{VAULT_SERVER_PUB_FILENAME} {VAULT_CA_PUB_FILENAME} {VAULT_CA_PRIV_FILENAME} \
                cert.conf server.csr {}",
            VAULT_CA_PUB_FILENAME.replace(".pem", ".srl"),
        )
        .run_with(&sudo_set)?;

        // Update Vault configuration file
        let vault_token: String = registry
            .get(format!("$/clusters/{cluster}/consul/tokens/vault"))?
            .try_into()?;

        let token_sed = format!(
            r#"s/^( *token = ")temporary_token(")$/\1{}\2/"#,
            vault_token.replace("/", r"\/")
        );
        let vault_config_path = "/etc/vault.d/vault.hcl";
        sed!("-ri {token_sed} {vault_config_path}").run_with(&vault_set)?;

        // Start Vault service.
        systemctl!("enable vault").run_with(&sudo_set)?;
        systemctl!("start vault").run_with(&sudo_set)?;

        let unseal_keys_key = format!("$/clusters/{cluster}/vault/unseal_keys");
        let unseal_keys = match registry
            .get(&unseal_keys_key)
            .and_then(Vec::<String>::try_from)
        {
            Ok(unseal_keys) => unseal_keys,
            Err(kv::Error::KeyDoesNotExist(_)) => {
                let (_, vault_init_output) = vault!("operator init")
                    .settings(&common_set)
                    .hide_output()
                    .run()?;

                let mut unseal_keys = Vec::new();
                let unseal_key_prefix = "Unseal Key ";
                let root_key_prefix = "Initial Root Token";
                let vault_init_re = Regex::new(&format!(
                    r"(?P<key>{unseal_key_prefix}\d+|{root_key_prefix}): *(?P<value>.*)"
                ))
                .unwrap();

                for cap in vault_init_re.captures_iter(&vault_init_output) {
                    let value = &cap["value"];
                    match &cap["key"] {
                        key if key == root_key_prefix => {
                            registry.put(format!("{unseal_keys_key}/root_token"), value)?;
                        }
                        key if key.starts_with(unseal_key_prefix) => {
                            let index = key
                                .strip_prefix(unseal_key_prefix)
                                .unwrap()
                                .parse::<usize>()
                                .unwrap()
                                - 1;
                            registry.put(format!("{unseal_keys_key}/{index}"), value)?;
                            if index >= unseal_keys.len() {
                                unseal_keys.extend(
                                    std::iter::repeat(String::new())
                                        .take(index + 1 - unseal_keys.len()),
                                );
                            }
                            unseal_keys[index] = value.to_string();
                        }
                        _ => unreachable!(),
                    }
                }
                unseal_keys
            }
            Err(err) => return Err(err.into()),
        };

        for unseal_key in unseal_keys.into_iter().take(3) {
            vault!("operator unseal {unseal_key}").run_with(&common_set)?;
        }

        Ok(InitializeNomad)
    }

    #[define_commands(
        cfssl, cfssljson, chown, complete, mkdir, mv, nomad, rm, sed, systemctl, tee
    )]
    fn initialize_nomad(proc: &mut DeployNode, registry: &impl WriteStore) -> Result<Self> {
        let cluster = &proc.cluster;
        let password = proc.get_password();
        let client = proc.connect(registry, ssh::Options::default())?;
        let common_set = process::Settings::default().ssh(&client);
        let sudo_set = common_set.clone().sudo_password(&*password);
        let nomad_set = sudo_set.clone().sudo_user("nomad");

        // Set up command autocomplete.
        nomad!("-autocomplete-install")
            .settings(&common_set)
            .success_codes([0, 1])
            .run()?;
        complete!("-C {BIN_DIR}/nomad nomad").run_with(&common_set)?;

        mkdir!("-p {NOMAD_CERTS_DIR}").run_with(&sudo_set)?;

        // Generate common cluster key.
        let registry_key = format!("$/clusters/{cluster}/nomad/key");
        let encrypt_key: String = match registry.get(&registry_key).and_then(String::try_from) {
            Ok(key) => key,
            Err(kv::Error::KeyDoesNotExist(_)) => {
                let (_, key) = nomad!("operator keygen")
                    .settings(&common_set)
                    .hide_stdout()
                    .run()?;
                registry.put(&registry_key, key.clone())?;
                key
            }
            Err(err) => return Err(err.into()),
        };

        // Create or distribute certificate authority certificate.
        let ca_pub_key = format!("$/clusters/{cluster}/nomad/certs/ca_pub");
        let ca_priv_key = format!("$/clusters/{cluster}/nomad/certs/ca_priv");
        let ca_pub_path = format!("{NOMAD_CERTS_DIR}/{NOMAD_CA_PUB_FILENAME}");
        let ca_priv_path = format!("{NOMAD_CERTS_DIR}/{NOMAD_CA_PRIV_FILENAME}");
        match registry.get(&ca_pub_key).and_then(kv::FileRef::try_from) {
            Ok(ca_pub_file_ref) => {
                Self::upload_certificates(
                    client,
                    &*password,
                    ca_pub_file_ref.path(),
                    &ca_pub_path,
                    registry
                        .get(ca_priv_key)
                        .and_then(kv::FileRef::try_from)?
                        .path(),
                    &ca_priv_path,
                )?;
            }
            Err(kv::Error::KeyDoesNotExist(_)) => {
                // Create CA certificate.
                let (_, csr) = cfssl!("print-defaults csr").run_with(&common_set)?;
                let (_, json_cert) = cfssl!("gencert -initca -")
                    .settings(&common_set)
                    .stdin_lines(csr.lines())
                    .hide_stdout()
                    .run()?;
                cfssljson!("-bare nomad-ca")
                    .settings(&common_set)
                    .stdin_lines(json_cert.lines())
                    .run()?;
                mv!("{NOMAD_CA_PUB_FILENAME} {NOMAD_CA_PRIV_FILENAME} {NOMAD_CERTS_DIR}")
                    .run_with(&sudo_set)?;

                // Remove extraneous files.
                rm!("nomad-ca.csr").run_with(&sudo_set)?;

                // Download certificate and key.
                Self::download_certificates(
                    registry,
                    client,
                    &*password,
                    &ca_pub_path,
                    &ca_priv_path,
                    &ca_pub_key,
                    &ca_priv_key,
                )?;
            }
            Err(err) => return Err(err.into()),
        }
        chown!("-R nomad:nomad {NOMAD_CERTS_DIR}").run_with(&sudo_set)?;

        tee!("cfssl.json")
            .settings(&common_set)
            .stdin_lines(include_str!("../../config/nomad/cfssl.json").lines())
            .run()?;

        // Create server certificates.
        let (_, json_cert) = cfssl!(
            "gencert -ca={ca_pub_path} -ca-key={ca_priv_path} -config=cfssl.json \
                -hostname=server.global.nomad,localhost,127.0.0.1 -"
        )
        .settings(&nomad_set)
        .stdin_line("{}")
        .hide_stdout()
        .run()?;
        cfssljson!("-bare server")
            .settings(&sudo_set)
            .stdin_lines(json_cert.lines())
            .run()?;
        mv!("{NOMAD_SERVER_PUB_FILENAME} {NOMAD_SERVER_PRIV_FILENAME} {NOMAD_CERTS_DIR}")
            .run_with(&sudo_set)?;
        chown!("-R nomad:nomad {NOMAD_CERTS_DIR}").run_with(&sudo_set)?;

        // Create client certificates.
        let (_, json_cert) = cfssl!(
            "gencert -ca={ca_pub_path} -ca-key={ca_priv_path} -config=cfssl.json \
                -hostname=client.global.nomad,localhost,127.0.0.1 -",
        )
        .settings(&nomad_set)
        .stdin_line("{}")
        .hide_stdout()
        .run()?;
        cfssljson!("-bare client")
            .settings(&sudo_set)
            .stdin_lines(json_cert.lines())
            .run()?;
        mv!("{NOMAD_CLIENT_PUB_FILENAME} {NOMAD_CLIENT_PRIV_FILENAME} {NOMAD_CERTS_DIR}")
            .run_with(&sudo_set)?;
        chown!("-R nomad:nomad {NOMAD_CERTS_DIR}").run_with(&sudo_set)?;

        // Create CLI certificates.
        let (_, json_cert) =
            cfssl!("gencert -ca={ca_pub_path} -ca-key={ca_priv_path} -profile=client -")
                .settings(&nomad_set)
                .stdin_line("{}")
                .hide_stdout()
                .run()?;
        cfssljson!("-bare cli")
            .settings(&sudo_set)
            .stdin_lines(json_cert.lines())
            .run()?;
        mv!("{NOMAD_CLI_PUB_FILENAME} {NOMAD_CLI_PRIV_FILENAME} {NOMAD_CERTS_DIR}")
            .run_with(&sudo_set)?;
        chown!("-R nomad:nomad {NOMAD_CERTS_DIR}").run_with(&sudo_set)?;

        // Remove extraneous files.
        rm!("{ca_priv_path} cfssl.json server.csr client.csr cli.csr").run_with(&sudo_set)?;

        // Update Nomad configuration file
        let encrypt_sed = format!(
            r#"s/^( *encrypt = ")temporary_key(")$/\1{}\2/"#,
            encrypt_key.replace("/", r"\/")
        );
        let server_config_path = "/etc/nomad.d/server.hcl";
        sed!("-ri {encrypt_sed} {server_config_path}").run_with(&nomad_set)?;

        // Start Nomad service.
        systemctl!("enable nomad").run_with(&sudo_set)?;
        systemctl!("start nomad").run_with(&sudo_set)?;

        Ok(SetUpNomadAcl)
    }

    #[define_commands(nomad, rm, tee)]
    fn set_up_nomad_acl(proc: &mut DeployNode, registry: &impl WriteStore) -> Result<()> {
        let cluster = &proc.cluster;
        let password = proc.get_password();

        let client = proc.connect(registry, ssh::Options::default())?;
        let mut common_set = process::Settings::default().ssh(&client);
        let mut nomad_set = common_set
            .clone()
            .env("NOMAD_ADDR", "https://localhost:4646")
            .env(
                "NOMAD_CACERT",
                format!("{NOMAD_CERTS_DIR}/{NOMAD_CA_PUB_FILENAME}"),
            )
            .env(
                "NOMAD_CLIENT_CERT",
                format!("{NOMAD_CERTS_DIR}/{NOMAD_CLI_PUB_FILENAME}"),
            )
            .env(
                "NOMAD_CLIENT_KEY",
                format!("{NOMAD_CERTS_DIR}/{NOMAD_CLI_PRIV_FILENAME}"),
            )
            .sudo_user("nomad")
            .sudo_password(&*password);

        // Set up ACL.
        let mgmt_token_key = format!("$/clusters/{cluster}/nomad/tokens/management");
        let access_token_key = format!("$/clusters/{cluster}/nomad/tokens/accessor");
        match registry.get(&mgmt_token_key) {
            Ok(_) => (),
            Err(kv::Error::KeyDoesNotExist(_)) => {
                // Bootstrap ACL.
                let (_, output) = nomad!("acl bootstrap")
                    .settings(&nomad_set)
                    .hide_stdout()
                    .run()?;
                let mgmt_token = Self::get_id("Secret ID", &output);
                let access_token = Self::get_id("Accessor ID", &output);

                common_set = common_set.secret(mgmt_token);
                common_set = common_set.secret(access_token);
                nomad_set = nomad_set.secret(mgmt_token);
                nomad_set = nomad_set.secret(access_token);

                // Create ACL policy.
                tee!("anonymous-policy.hcl")
                    .settings(&common_set)
                    .stdin_lines(include_str!("../../config/nomad/anonymous-policy.hcl").lines())
                    .run()?;
                nomad!(
                    "acl policy apply -description='Anonymous policy (full-access)' \
                        -token={mgmt_token} anonymous anonymous-policy.hcl",
                )
                .run_with(&nomad_set)?;
                rm!("anonymous-policy.hcl").run_with(&common_set)?;

                registry.put(mgmt_token_key, mgmt_token)?;
                registry.put(access_token_key, access_token)?;
            }
            Err(err) => return Err(err.into()),
        }

        Ok(())
    }
}

impl DeployNodeState {
    fn get_id<'a>(key: &str, output: &'a str) -> &'a str {
        Regex::new(&format!(r"{} *(:|=) *([\w-]*)", regex::escape(key)))
            .unwrap()
            .captures(output)
            .unwrap()
            .get(2)
            .unwrap()
            .as_str()
    }

    #[define_commands(tee)]
    fn upload_certificates(
        client: &ssh::Client,
        password: &str,
        cert_pub_source: &Path,
        cert_pub_dest: &impl AsRef<OsStr>,
        cert_priv_source: &Path,
        cert_priv_dest: &impl AsRef<OsStr>,
    ) -> Result<()> {
        // Read certificate and key files.
        let mut cert_pub_file = File::open(cert_pub_source)?;
        let mut cert_priv_file = File::open(cert_priv_source)?;
        let mut cert_pub = String::new();
        let mut cert_priv = String::new();
        cert_pub_file.read_to_string(&mut cert_pub)?;
        cert_priv_file.read_to_string(&mut cert_priv)?;

        // Send certificate and key to server.
        tee!("{}", cert_pub_dest.as_ref().to_string_lossy())
            .stdin_lines(cert_pub.lines())
            .sudo_password(password)
            .ssh(client)
            .run()?;
        tee!("{}", cert_priv_dest.as_ref().to_string_lossy())
            .stdin_lines(cert_priv.lines())
            .sudo_password(password)
            .hide_stdout()
            .ssh(client)
            .run()?;

        Ok(())
    }

    #[define_commands(cat)]
    fn download_certificates(
        registry: &impl WriteStore,
        client: &ssh::Client,
        password: &str,
        cert_pub_path: &str,
        cert_priv_path: &str,
        cert_pub_key: &str,
        cert_priv_key: &str,
    ) -> Result<()> {
        // Download certificate and key.
        let (_, cert_pub) = cat!("{cert_pub_path}")
            .sudo_password(password)
            .ssh(client)
            .run()?;
        let (_, cert_priv) = cat!("{cert_priv_path}")
            .hide_stdout()
            .sudo_password(password)
            .ssh(client)
            .run()?;

        // Store certificate and key in registry.
        let cert_pub_file_ref = registry.create_file(cert_pub_key)?;
        let cert_priv_file_ref = registry.create_file(cert_priv_key)?;
        let mut cert_pub_file = File::options().write(true).open(cert_pub_file_ref.path())?;
        let mut cert_priv_file = File::options()
            .write(true)
            .open(cert_priv_file_ref.path())?;
        cert_pub_file.write_all(cert_pub.as_bytes())?;
        cert_priv_file.write_all(cert_priv.as_bytes())?;

        Ok(())
    }

    fn numeral(n: u64) -> Cow<'static, str> {
        match n {
            0 => "zero".into(),
            1 => "one".into(),
            2 => "two".into(),
            3 => "three".into(),
            4 => "four".into(),
            5 => "five".into(),
            6 => "six".into(),
            7 => "seven".into(),
            8 => "eight".into(),
            9 => "nine".into(),
            10 => "ten".into(),
            11 => "eleven".into(),
            12 => "twelve".into(),
            13 => "thirteen".into(),
            14 => "fourteen".into(),
            15 => "fifteen".into(),
            16 => "sixteen".into(),
            17 => "seventeen".into(),
            18 => "eighteen".into(),
            19 => "nineteen".into(),
            20 => "twenty".into(),
            30 => "thirty".into(),
            40 => "fourty".into(),
            50 => "fifty".into(),
            60 => "sixty".into(),
            70 => "seventy".into(),
            80 => "eighty".into(),
            90 => "ninety".into(),
            100 => "hundred".into(),
            1000 => "thousand".into(),
            1_000_000 => "million".into(),
            n if n <= 99 => {
                format!("{}-{}", Self::numeral(n - n % 10), Self::numeral(n % 10)).into()
            }
            n if n <= 199 => format!("hundred-{}", Self::numeral(n % 100)).into(),
            n if n <= 999 && n % 100 == 0 => format!("{}-hundred", Self::numeral(n / 100)).into(),
            n if n <= 999 => {
                format!("{}-{}", Self::numeral(n - n % 100), Self::numeral(n % 100)).into()
            }
            n if n <= 1999 => format!("thousand-{}", Self::numeral(n % 1000)).into(),
            n if n <= 999_999 && n % 1000 == 0 => {
                format!("{}-thousand", Self::numeral(n / 1000)).into()
            }
            n if n <= 999_999 => format!(
                "{}-{}",
                Self::numeral(n - n % 1000),
                Self::numeral(n % 1000)
            )
            .into(),
            n if n <= 1_999_999 => format!("million-{}", Self::numeral(n % 1_000_000)).into(),
            n if n % 1_000_000 == 0 => format!("{}-million", Self::numeral(n / 1_000_000)).into(),

            mut n => {
                let mut list = Vec::new();
                loop {
                    list.push(Self::numeral(n % 1_000_000));
                    n /= 1_000_000;
                    if n == 0 {
                        break;
                    }
                    list.push("million".into());
                }
                list.reverse();
                list.join("-".into()).into()
            }
        }
    }
}
