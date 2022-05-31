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
use osshkeys::{keys::FingerprintHash, PublicKey, PublicParts};
use regex::Regex;
use serde::{Deserialize, Serialize};
use structopt::StructOpt;

use hoc_core::{
    cmd,
    kv::{self, ReadStore, WriteStore},
    process::{self, ssh},
};
use hoc_log::{bail, choose, error, hidden_input, info, prompt, status, LogErr, Result};
use hoc_macros::{Procedure, ProcedureState};
use xz2::read::XzDecoder;
use zip::ZipArchive;

use crate::command::util::{cidr::Cidr, disk, os::OperatingSystem};

const NOMAD_URL: &str = "https://releases.hashicorp.com/nomad/1.2.6/nomad_1.2.6_linux_arm64.zip";
const CONSUL_URL: &str =
    "https://releases.hashicorp.com/consul/1.11.4/consul_1.11.4_linux_arm64.zip";
const ENVOY_URL: &str =
    "https://archive.tetratelabs.io/envoy/download/v1.22.0/envoy-v1.22.0-linux-arm64.tar.xz";
const GO_URL: &str = "https://go.dev/dl/go1.18.2.linux-arm64.tar.gz";
const CFSSL_PKG_PATH: &str = "github.com/cloudflare/cfssl/cmd/cfssl";
const CFSSLJSON_PKG_PATH: &str = "github.com/cloudflare/cfssl/cmd/cfssljson";

const NOMAD_VERSION: &str = "1.2.6";
const CONSUL_VERSION: &str = "1.11.4";
const ENVOY_VERSION: &str = "1.22.0";
const GO_VERSION: &str = "1.18.2";
const CFSSL_VERSION: &str = "v1.6.1";

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

    fn get_password(&self) -> Result<String> {
        if self.password.borrow().is_none() {
            let password = hidden_input!("[admin] Password").get();
            self.password.replace(Some(password));
        }

        Ok(self.password.borrow().clone().unwrap())
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
            options.password.replace(self.get_password()?);
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
    Mount,
    #[state(transient)]
    ModifyRaspberryPiOsImage {
        disk_partition_id: String,
    },
    #[state(transient)]
    ModifyUbuntuImage {
        disk_partition_id: String,
    },
    Unmount {
        disk_partition_id: String,
    },
    AwaitNode,

    AddNewUser,
    AssignSudoPrivileges,
    DeletePiUser,
    SetUpSshAccess,
    ChangePassword,

    InstallDependencies,
    InitializeNomad,
    SetUpNomadAcl,
    InitializeConsul,
    #[state(finish)]
    SetUpConsulAcl,
}

impl Run for DeployNodeState {
    fn download_image(proc: &mut DeployNode, registry: &impl WriteStore) -> Result<Self> {
        let os = proc.node_os;

        let file_ref = match registry.create_file(format!("$/images/{os}")) {
            Ok(file_ref) => status!("Download image").on(|| {
                let image_url = os.image_url();
                info!("URL: {}", image_url);

                let mut file = File::options().write(true).open(file_ref.path())?;

                reqwest::blocking::get(image_url)
                    .log_err()?
                    .copy_to(&mut file)
                    .log_err()?;

                Result::Ok(file_ref)
            })?,
            Err(kv::Error::KeyAlreadyExists(_)) => {
                info!("Using cached file");
                return Ok(FlashImage);
            }
            Err(error) => return Err(error.into()),
        };

        status!("Determine file type").on(|| {
            let output = cmd!("file", file_ref.path()).run()?.1.to_lowercase();
            if output.contains("zip archive") {
                info!("Zip archive file type detected");
                Ok(DecompressZipArchive)
            } else if output.contains("xz compressed data") {
                info!("XZ compressed data file type detected");
                Ok(DecompressXzFile)
            } else {
                error!("Unsupported file type")?.into()
            }
        })
    }

    fn decompress_zip_archive(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        let (image_data, mut image_file) = status!("Read ZIP archive").on(|| {
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
                    status!("Decompress image").on(|| {
                        archive_file
                            .read_to_end(&mut data)
                            .log_context("Failed to read image in ZIP archive")?;
                        buf.replace(data);

                        Result::Ok(())
                    })?;
                    break;
                }
            }

            if let Some(data) = buf {
                Result::Ok((data, file))
            } else {
                bail!("Image not found within ZIP archive");
            }
        })?;

        status!("Save decompressed image to file").on(|| {
            image_file.seek(SeekFrom::Start(0))?;
            image_file.set_len(0)?;
            image_file.write_all(&image_data)?;

            Result::Ok(())
        })?;

        Ok(FlashImage)
    }

    fn decompress_xz_file(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        let (image_data, mut image_file) = status!("Read XZ file").on(|| {
            let file_path = proc.get_os_image_path(registry)?;
            let file = File::options().read(true).write(true).open(&file_path)?;

            let mut decompressor = XzDecoder::new(&file);

            let mut buf = Vec::new();
            status!("Decompress image").on(|| {
                decompressor
                    .read_to_end(&mut buf)
                    .log_context("Failed to read image in XZ file")
            })?;

            Result::Ok((buf, file))
        })?;

        status!("Save decompressed image to file").on(|| {
            image_file.seek(SeekFrom::Start(0))?;
            image_file.set_len(0)?;
            image_file.write_all(&image_data)?;

            Result::Ok(())
        })?;

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
        let used_addresses: Vec<IpAddr> = registry
            .get_matches(format!("clusters/{cluster}/nodes/*/network/address"))?
            .into_iter()
            .map(|item| String::try_from(item)?.parse().log_err())
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
            format!("server.{inc}"),
        )?;

        Ok(FlashImage)
    }

    fn flash_image(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        let os = proc.node_os;

        let mut disks: Vec<_> = disk::get_attached_disks()
            .log_context("Failed to get attached disks")?
            .collect();

        let index = if disks.len() == 1 {
            0
        } else {
            choose!("Which disk is your SD card?").items(&disks).get()?
        };

        let disk = disks.remove(index);
        status!("Unmount SD card").on(|| cmd!("diskutil", "unmountDisk", disk.id).run())?;

        status!("Flash SD card").on(|| {
            let image_path = proc.get_os_image_path(registry)?;

            prompt!(
                "Do you want to flash target disk '{}' with operating system '{os}'?",
                disk.description(),
            )
            .get()?;

            cmd!(
                "dd",
                "bs=1m",
                format!("if={}", image_path.to_string_lossy()),
                format!("of=/dev/r{}", disk.id),
            )
            .sudo()
            .run()?;

            Result::Ok(())
        })?;

        Ok(Mount)
    }

    fn mount(proc: &mut DeployNode, _registry: &impl ReadStore) -> Result<Self> {
        let os = proc.node_os;

        let boot_partition_name = match os {
            OperatingSystem::RaspberryPiOs { .. } => "boot",
            OperatingSystem::Ubuntu { .. } => "system-boot",
        };

        let disk_partition_id = status!("Mount boot partition").on(|| {
            let mut partitions: Vec<_> = disk::get_attached_disk_partitions()
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

            cmd!("diskutil", "mount", disk_partition.id).run()?;

            Result::Ok(disk_partition.id)
        })?;

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

        status!("Configure image")
            .on(|| status!("Create SSH file").on(|| File::create(mount_dir.join("ssh"))))?;

        Ok(Unmount { disk_partition_id })
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

        let pub_key = status!("Read SSH keypair").on(|| {
            let pub_key = fs::read_to_string(pub_key_path)?;
            info!(
                "SSH public key fingerprint randomart:\n{}",
                PublicKey::from_keystr(&pub_key)
                    .log_err()?
                    .fingerprint_randomart(FingerprintHash::SHA256)
                    .log_err()?
            );

            Result::Ok(pub_key)
        })?;

        let mount_dir = disk::find_mount_dir(&disk_partition_id)?;

        status!("Prepare image initialization").on(|| {
            let data_map: serde_yaml::Value = serde_yaml::from_str(&format!(
                include_str!("../../config/user-data"),
                admin_username = username,
                cluster = cluster,
                hostname = proc.node_name,
                nomad_name = nomad_name,
                ip_address = address,
                ssh_pub_key = pub_key,
            ))
            .log_context("invalid user-data format")?;

            let data = serde_yaml::to_string(&data_map).log_err()?;
            let data = "#cloud-config".to_string() + data.strip_prefix("---").unwrap_or(&data);
            info!(
                "Updating {} with the following configuration:\n{}",
                "/user-data".blue(),
                data
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

            Result::Ok(())
        })?;

        Ok(Unmount { disk_partition_id })
    }

    fn unmount(
        _proc: &mut DeployNode,
        _registry: &impl ReadStore,
        disk_partition_id: String,
    ) -> Result<Self> {
        status!("Sync image disk writes").on(|| cmd!("sync").run())?;
        status!("Unmount image disk")
            .on(|| cmd!("diskutil", "unmount", disk_partition_id).run())?;

        Ok(AwaitNode)
    }

    fn await_node(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        let os = proc.node_os;
        let address = proc.get_address(registry)?;

        info!(
            "The SD card has now been prepared. Take out the SD card and insert it into the \
            node hardware. Then, plug it in to the network via ethernet and power it on."
        );
        prompt!("Have you prepared the node hardware?").get()?;

        cmd!("ping", "-o", "-t", "300", "-i", "5", address.to_string()).run()?;

        let state = match os {
            OperatingSystem::RaspberryPiOs { .. } => AddNewUser,
            OperatingSystem::Ubuntu { .. } => {
                let client = proc.connect(registry, ssh::Options::default())?;
                cmd!("cloud-init", "status", "--wait").ssh(&client).run()?;
                ChangePassword
            }
        };

        Ok(state)
    }

    fn add_new_user(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        let username: String = proc.get_user(registry)?;
        let password = proc.get_password()?;
        let client = proc.connect(
            registry,
            ssh::Options::default().user("pi").password("raspberry"),
        )?;

        // Add the new user.
        cmd!("adduser", username)
            .stdin_lines([&*password, &*password])
            .sudo()
            .ssh(&client)
            .run()?;

        Ok(AssignSudoPrivileges)
    }

    fn assign_sudo_privileges(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        let username: String = proc.get_user(registry)?;
        let client = proc.connect(
            registry,
            ssh::Options::default().user("pi").password("raspberry"),
        )?;
        let common_set = process::Settings::default().sudo().ssh(&client);

        // Assign the user the relevant groups.
        cmd!(
            "usermod",
            "-a",
            "-G",
            "adm,dialout,cdrom,sudo,audio,video,plugdev,games,users,input,netdev,gpio,i2c,spi",
            username,
        )
        .run_with(&common_set)?;

        // Create sudo file for the user.
        let sudo_file = format!("/etc/sudoers.d/010_{username}");
        cmd!("tee", sudo_file)
            .settings(&common_set)
            .stdin_line(&format!("{username} ALL=(ALL) PASSWD: ALL"))
            .hide_output()
            .run()?;
        cmd!("chmod", "440", sudo_file).run_with(&common_set)?;

        Ok(DeletePiUser)
    }

    fn delete_pi_user(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        let password = proc.get_password()?;
        let client = proc.connect(registry, ssh::Options::default().password_auth())?;
        let common_set = process::Settings::default()
            .sudo_password(&*password)
            .ssh(&client);

        // Kill all processes owned by the `pi` user.
        cmd!("pkill", "-u", "pi")
            .settings(&common_set)
            .success_codes([0, 1])
            .run()?;

        // Delete the default `pi` user.
        cmd!("deluser", "--remove-home", "pi").run_with(&common_set)?;

        Ok(SetUpSshAccess)
    }

    fn set_up_ssh_access(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        let username = proc.get_user(registry)?;
        let password = proc.get_password()?;
        let pub_path = proc.get_pub_key_path(registry)?;
        let client = proc.connect(registry, ssh::Options::default().password_auth())?;
        let common_set = process::Settings::default().ssh(&client);
        let sudo_set = common_set.clone().sudo_password(&*password);

        let pub_key = status!("Read SSH keypair").on(|| {
            let pub_key = fs::read_to_string(&pub_path)?;
            info!(
                "SSH public key fingerprint randomart:\n{}",
                PublicKey::from_keystr(&pub_key)
                    .log_err()?
                    .fingerprint_randomart(FingerprintHash::SHA256)
                    .log_err()?
            );

            Result::Ok(pub_key)
        })?;

        status!("Send SSH public key").on(|| {
            // Create the `.ssh` directory.
            cmd!("mkdir", "-p", "-m", "700", format!("/home/{username}/.ssh"))
                .run_with(&common_set)?;

            let authorized_keys_path = format!("/home/{username}/.ssh/authorized_keys");

            // Check if the authorized keys file exists.
            let (status_code, _) = cmd!("test", "-s", authorized_keys_path)
                .settings(&common_set)
                .success_codes([0, 1])
                .run()?;
            if status_code == 1 {
                // Create the authorized keys file.
                cmd!("tee", authorized_keys_path)
                    .settings(&common_set)
                    .stdin_line(&username)
                    .run()?;
                cmd!("chmod", "644", authorized_keys_path).run_with(&common_set)?;
            }

            // Copy the public key to the authorized keys file.
            let key = pub_key.replace("/", r"\/");
            cmd!(
                "sed",
                "-i",
                format!(
                    "0,/{username}$/{{h;s/^.*{username}$/{key}/}};${{x;/^$/{{s//{key}/;H}};x}}"
                ),
                authorized_keys_path,
            )
            .settings(&common_set)
            .secret(&key)
            .run()?;

            Result::Ok(())
        })?;

        status!("Initialize SSH server").on(|| {
            let sshd_config_path = "/etc/ssh/sshd_config";

            // Set `PasswordAuthentication` to `no`.
            let key = "PasswordAuthentication";
            cmd!(
                "sed",
                "-i",
                format!("0,/{key}/{{h;s/^.*{key}.*$/{key} no/}};${{x;/^$/{{s//{key} no/;H}};x}}"),
                sshd_config_path,
            )
            .settings(&common_set)
            .run()?;

            // Verify sshd config and restart the SSH server.
            cmd!("sshd", "-t").run_with(&sudo_set)?;
            cmd!("systemctl", "restart", "ssh").run_with(&sudo_set)?;

            // Verify again after SSH server restart.
            let client = proc.connect(registry, ssh::Options::default())?;

            cmd!("sshd", "-t").settings(&sudo_set).ssh(&client).run()?;

            Result::Ok(())
        })?;

        Ok(InstallDependencies)
    }

    fn change_password(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        let username = proc.get_user(registry)?;
        let password = proc.get_password()?;
        let client = proc.connect(registry, ssh::Options::default())?;

        cmd!("chpasswd")
            .sudo_password("temporary_password")
            .stdin_line(format!("{username}:{password}"))
            .ssh(&client)
            .run()?;

        Ok(InstallDependencies)
    }

    fn install_dependencies(proc: &mut DeployNode, registry: &impl ReadStore) -> Result<Self> {
        let password = proc.get_password()?;
        let client = proc.connect(registry, ssh::Options::default())?;
        let common_set = process::Settings::default().ssh(&client);
        let sudo_set = common_set.clone().sudo_password(&*password);

        let local_dir = "/usr/local";
        let bin_dir = "/usr/local/bin";
        let nomad_path = format!("/run/nomad/{NOMAD_VERSION}.zip");
        let consul_path = format!("/run/consul/{CONSUL_VERSION}.zip");
        let envoy_path = format!("/run/envoy/{ENVOY_VERSION}.tar.xz");
        let go_path = format!("/run/go/{GO_VERSION}.tar.xz");

        status!("Nomad").on(|| {
            cmd!("wget", "--no-verbose", NOMAD_URL, "-O", nomad_path).run_with(&sudo_set)?;
            cmd!("unzip", "-o", nomad_path, "-d", bin_dir).run_with(&sudo_set)
        })?;

        status!("Consul").on(|| {
            cmd!("wget", "--no-verbose", CONSUL_URL, "-O", consul_path).run_with(&sudo_set)?;
            cmd!("unzip", "-o", consul_path, "-d", bin_dir).run_with(&sudo_set)
        })?;

        status!("Envoy").on(|| {
            cmd!("wget", "--no-verbose", ENVOY_URL, "-O", envoy_path).run_with(&sudo_set)?;
            cmd!(
                "tar",
                "-xJf",
                envoy_path,
                "--strip-components",
                "2",
                "-C",
                bin_dir,
            )
            .run_with(&sudo_set)
        })?;

        status!("Go").on(|| {
            cmd!("wget", "--no-verbose", GO_URL, "-O", go_path).run_with(&sudo_set)?;
            cmd!("tar", "-xzf", go_path, "-C", local_dir).run_with(&sudo_set)
        })?;

        status!("cfssl").on(|| {
            cmd!("go", "install", format!("{CFSSL_PKG_PATH}@{CFSSL_VERSION}"))
                .run_with(&common_set)?;
            cmd!(
                "go",
                "install",
                format!("{CFSSLJSON_PKG_PATH}@{CFSSL_VERSION}")
            )
            .run_with(&common_set)?;
            cmd!("mv", "go/bin/cfssl", "go/bin/cfssljson", bin_dir).run_with(&sudo_set)
        })?;

        Ok(InitializeNomad)
    }

    fn initialize_nomad(proc: &mut DeployNode, registry: &impl WriteStore) -> Result<Self> {
        let cluster = &proc.cluster;
        let password = proc.get_password()?;
        let client = proc.connect(registry, ssh::Options::default())?;
        let common_set = process::Settings::default().ssh(&client);
        let sudo_set = common_set.clone().sudo_password(&*password);
        let nomad_set = sudo_set.clone().sudo_user(&"nomad");

        // Set up command autocomplete.
        cmd!("nomad", "-autocomplete-install")
            .settings(&common_set)
            .success_codes([0, 1])
            .run()?;
        cmd!("complete", "-C", "/usr/local/bin/nomad", "nomad").run_with(&common_set)?;

        cmd!("mkdir", "-p", NOMAD_CERTS_DIR).run_with(&sudo_set)?;

        // Generate common cluster key.
        let registry_key = format!("$/clusters/{cluster}/nomad/key");
        let encrypt_key: String = match registry.get(&registry_key).and_then(String::try_from) {
            Ok(key) => key,
            Err(kv::Error::KeyDoesNotExist(_)) => {
                let (_, key) = cmd!("nomad", "operator", "keygen")
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
                let (_, csr) = cmd!("cfssl", "print-defaults", "csr").run_with(&common_set)?;
                let (_, json_cert) = cmd!("cfssl", "gencert", "-initca", "-")
                    .settings(&common_set)
                    .stdin_lines(csr.lines())
                    .hide_stdout()
                    .run()?;
                cmd!("cfssljson", "-bare", "nomad-ca")
                    .settings(&common_set)
                    .stdin_lines(json_cert.lines())
                    .run()?;
                cmd!(
                    "mv",
                    NOMAD_CA_PUB_FILENAME,
                    NOMAD_CA_PRIV_FILENAME,
                    NOMAD_CERTS_DIR
                )
                .run_with(&sudo_set)?;

                // Remove extraneous files.
                cmd!("rm", "nomad-ca.csr").run_with(&sudo_set)?;

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
        cmd!("chown", "-R", "nomad:nomad", NOMAD_CERTS_DIR).run_with(&sudo_set)?;

        cmd!("tee", "cfssl.json")
            .settings(&common_set)
            .stdin_lines(include_str!("../../config/nomad/cfssl.json").lines())
            .run()?;

        // Create server certificates.
        let (_, json_cert) = cmd!(
            "cfssl",
            "gencert",
            "-ca",
            ca_pub_path,
            "-ca-key",
            ca_priv_path,
            "-config",
            "cfssl.json",
            "-hostname",
            "server.global.nomad,localhost,127.0.0.1",
            "-"
        )
        .settings(&nomad_set)
        .stdin_line("{}")
        .hide_stdout()
        .run()?;
        cmd!("cfssljson", "-bare", "server")
            .settings(&sudo_set)
            .stdin_lines(json_cert.lines())
            .run()?;
        cmd!(
            "mv",
            NOMAD_SERVER_PUB_FILENAME,
            NOMAD_SERVER_PRIV_FILENAME,
            NOMAD_CERTS_DIR,
        )
        .run_with(&sudo_set)?;
        cmd!("chown", "-R", "nomad:nomad", NOMAD_CERTS_DIR).run_with(&sudo_set)?;

        // Create client certificates.
        let (_, json_cert) = cmd!(
            "cfssl",
            "gencert",
            "-ca",
            ca_pub_path,
            "-ca-key",
            ca_priv_path,
            "-config",
            "cfssl.json",
            "-hostname",
            "client.global.nomad,localhost,127.0.0.1",
            "-"
        )
        .settings(&nomad_set)
        .stdin_line("{}")
        .hide_stdout()
        .run()?;
        cmd!("cfssljson", "-bare", "client")
            .settings(&sudo_set)
            .stdin_lines(json_cert.lines())
            .run()?;
        cmd!(
            "mv",
            NOMAD_CLIENT_PUB_FILENAME,
            NOMAD_CLIENT_PRIV_FILENAME,
            NOMAD_CERTS_DIR,
        )
        .run_with(&sudo_set)?;
        cmd!("chown", "-R", "nomad:nomad", NOMAD_CERTS_DIR).run_with(&sudo_set)?;

        // Create CLI certificates.
        let (_, json_cert) = cmd!(
            "cfssl",
            "gencert",
            "-ca",
            ca_pub_path,
            "-ca-key",
            ca_priv_path,
            "-profile",
            "client",
            "-"
        )
        .settings(&nomad_set)
        .stdin_line("{}")
        .hide_stdout()
        .run()?;
        cmd!("cfssljson", "-bare", "cli")
            .settings(&sudo_set)
            .stdin_lines(json_cert.lines())
            .run()?;
        cmd!(
            "mv",
            NOMAD_CLI_PUB_FILENAME,
            NOMAD_CLI_PRIV_FILENAME,
            NOMAD_CERTS_DIR
        )
        .run_with(&sudo_set)?;
        cmd!("chown", "-R", "nomad:nomad", NOMAD_CERTS_DIR).run_with(&sudo_set)?;

        // Remove extraneous files.
        cmd!(
            "rm",
            ca_priv_path,
            "cfssl.json",
            "server.csr",
            "client.csr",
            "cli.csr"
        )
        .run_with(&sudo_set)?;

        // Update Nomad configuration file
        let encrypt_sed = format!(
            r#"s/^( *encrypt = ")temporary_key(")$/\1{}\2/"#,
            encrypt_key.replace("/", r"\/")
        );
        let server_config_path = "/etc/nomad.d/server.hcl";
        cmd!("sed", "-ri", encrypt_sed, server_config_path).run_with(&nomad_set)?;

        // Start Nomad service.
        cmd!("systemctl", "enable", "nomad").run_with(&sudo_set)?;
        cmd!("systemctl", "start", "nomad").run_with(&sudo_set)?;

        Ok(SetUpNomadAcl)
    }

    fn set_up_nomad_acl(proc: &mut DeployNode, registry: &impl WriteStore) -> Result<Self> {
        let cluster = &proc.cluster;
        let password = proc.get_password()?;

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
            .sudo_user(&"nomad")
            .sudo_password(&*password);

        // Set up ACL.
        let mgmt_token_key = format!("$/clusters/{cluster}/nomad/tokens/management");
        let access_token_key = format!("$/clusters/{cluster}/nomad/tokens/accessor");
        match registry.get(&mgmt_token_key) {
            Ok(_) => (),
            Err(kv::Error::KeyDoesNotExist(_)) => {
                // Bootstrap ACL.
                let (_, output) = cmd!("nomad", "acl", "bootstrap")
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
                cmd!("tee", "anonymous-policy.hcl")
                    .settings(&common_set)
                    .stdin_lines(include_str!("../../config/nomad/anonymous-policy.hcl").lines())
                    .run()?;
                cmd!(
                    "nomad",
                    "acl",
                    "policy",
                    "apply",
                    "-description",
                    "Anonymous policy (full-access)",
                    "-token",
                    mgmt_token,
                    "anonymous",
                    "anonymous-policy.hcl",
                )
                .run_with(&nomad_set)?;
                cmd!("rm", "anonymous-policy.hcl").run_with(&common_set)?;

                registry.put(mgmt_token_key, mgmt_token)?;
                registry.put(access_token_key, access_token)?;
            }
            Err(err) => return Err(err.into()),
        };

        Ok(InitializeConsul)
    }

    fn initialize_consul(proc: &mut DeployNode, registry: &impl WriteStore) -> Result<Self> {
        let cluster = &proc.cluster;
        let password = proc.get_password()?;
        let client = proc.connect(registry, ssh::Options::default())?;
        let common_set = process::Settings::default().ssh(&client);
        let sudo_set = common_set.clone().sudo_password(&*password);
        let consul_set = sudo_set.clone().sudo_user(&"consul");

        // Set up command autocomplete.
        cmd!("consul", "-autocomplete-install")
            .settings(&consul_set)
            .success_codes([0, 1])
            .run()?;
        cmd!("complete", "-C", "/usr/local/bin/consul", "consul").run_with(&common_set)?;

        // Generate common cluster key.
        let registry_key = format!("$/clusters/{cluster}/consul/key");
        let encrypt_key: String = match registry.get(&registry_key).and_then(String::try_from) {
            Ok(key) => key,
            Err(kv::Error::KeyDoesNotExist(_)) => {
                let (_, key) = cmd!("consul", "keygen")
                    .settings(&consul_set)
                    .hide_stdout()
                    .run()?;
                registry.put(&registry_key, key.clone())?;
                key
            }
            Err(err) => return Err(err.into()),
        };

        cmd!("mkdir", "-p", CONSUL_CERTS_DIR).run_with(&sudo_set)?;

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
                cmd!("consul", "tls", "ca", "create").run_with(&sudo_set)?;
                cmd!(
                    "mv",
                    CONSUL_CA_PUB_FILENAME,
                    CONSUL_CA_PRIV_FILENAME,
                    CONSUL_CERTS_DIR
                )
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
        cmd!("chown", "-R", "consul:consul", CONSUL_CERTS_DIR).run_with(&sudo_set)?;

        // Create server certificates.
        let cert_filename = format!("{cluster}-server-consul-0.pem");
        let key_filename = format!("{cluster}-server-consul-0-key.pem");
        cmd!(
            "consul",
            "tls",
            "cert",
            "create",
            "-server",
            "-dc",
            cluster,
            "-domain",
            "consul",
            "-ca",
            ca_pub_path,
            "-key",
            ca_priv_path,
        )
        .run_with(&sudo_set)?;
        cmd!("mv", cert_filename, key_filename, CONSUL_CERTS_DIR).run_with(&sudo_set)?;
        cmd!("chown", "-R", "consul:consul", CONSUL_CERTS_DIR).run_with(&sudo_set)?;

        // Remove CA key.
        cmd!("rm", ca_priv_path).run_with(&sudo_set)?;

        // Set or get auto-join address.
        let auto_join_key = format!("$/clusters/{cluster}/auto_join_address");
        let auto_join_address = match registry.get(&auto_join_key).and_then(String::try_from) {
            Ok(address) => address,
            Err(kv::Error::KeyDoesNotExist(_)) => {
                let address = proc.get_address(registry)?.to_string();
                registry.put(auto_join_key, address.clone())?;
                address
            }
            Err(err) => return Err(err.into()),
        };

        // Update Consul configuration file
        let encrypt_sed = format!(
            r#"s/^(encrypt = ")temporary_key(")$/\1{}\2/"#,
            encrypt_key.replace("/", r"\/")
        );
        let retry_join_sed =
            format!(r#"s/^(retry_join = \[")temporary_host("\])$/\1{auto_join_address}\2/"#);
        let consul_config_path = "/etc/consul.d/consul.hcl";
        cmd!("sed", "-ri", encrypt_sed, consul_config_path).run_with(&sudo_set)?;
        cmd!("sed", "-ri", retry_join_sed, consul_config_path).run_with(&sudo_set)?;

        // Validate Consul configuration.
        cmd!("consul", "validate", "/etc/consul.d/").run_with(&consul_set)?;

        // Start Consul service.
        cmd!("systemctl", "enable", "consul").run_with(&sudo_set)?;
        cmd!("systemctl", "start", "consul").run_with(&sudo_set)?;

        Ok(SetUpConsulAcl)
    }

    fn set_up_consul_acl(proc: &mut DeployNode, registry: &impl WriteStore) -> Result<()> {
        let cluster = &proc.cluster;

        let client = proc.connect(registry, ssh::Options::default())?;
        let mut common_set = process::Settings::default().ssh(&client);
        let mut consul_set = process::Settings::default().ssh(&client);

        // Set up ACL.
        let mgmt_token_key = format!("$/clusters/{cluster}/consul/tokens/management");
        let node_token_key = format!("$/clusters/{cluster}/consul/tokens/node");
        let (mgmt_token, node_token) =
            match registry.get(&mgmt_token_key).and_then(String::try_from) {
                Ok(token) => (
                    token,
                    registry.get(node_token_key).and_then(String::try_from)?,
                ),
                Err(kv::Error::KeyDoesNotExist(_)) => {
                    // Bootstrap ACL.
                    let (_, output) = cmd!("consul", "acl", "bootstrap")
                        .settings(&consul_set)
                        .hide_stdout()
                        .run()?;
                    let mgmt_token = Self::get_id("SecretID", &output).to_string();

                    common_set = common_set.secret(mgmt_token.clone());
                    consul_set = consul_set.secret(mgmt_token.clone());

                    // Create ACL policy.
                    cmd!("tee", "node-policy.hcl")
                        .settings(&common_set)
                        .stdin_lines(include_str!("../../config/consul/node-policy.hcl").lines())
                        .run()?;

                    cmd!(
                        "consul",
                        "acl",
                        "policy",
                        "create",
                        "-token",
                        mgmt_token,
                        "-name",
                        "node-policy",
                        "-rules",
                        "@node-policy.hcl"
                    )
                    .run_with(&consul_set)?;

                    cmd!("rm", "node-policy.hcl").run_with(&common_set)?;

                    // Create ACL token.
                    let (_, output) = cmd!(
                        "consul",
                        "acl",
                        "token",
                        "create",
                        "-token",
                        mgmt_token,
                        "-description",
                        "node token",
                        "-policy-name",
                        "node-policy"
                    )
                    .settings(&consul_set)
                    .hide_stdout()
                    .run()?;
                    let node_token = Self::get_id("SecretID", &output).to_string();

                    consul_set = consul_set.secret(node_token.clone());

                    registry.put(mgmt_token_key, mgmt_token.clone())?;
                    registry.put(node_token_key, node_token.clone())?;

                    (mgmt_token, node_token)
                }
                Err(err) => return Err(err.into()),
            };

        // Assign ACL token to node.
        cmd!(
            "consul",
            "acl",
            "set-agent-token",
            "-token",
            mgmt_token,
            "agent",
            node_token,
        )
        .run_with(&consul_set)?;

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
        cmd!("tee", cert_pub_dest)
            .stdin_lines(cert_pub.lines())
            .sudo_password(password)
            .ssh(client)
            .run()?;
        cmd!("tee", cert_priv_dest)
            .stdin_lines(cert_priv.lines())
            .sudo_password(password)
            .hide_stdout()
            .ssh(client)
            .run()?;

        Ok(())
    }

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
        let (_, cert_pub) = cmd!("cat", cert_pub_path)
            .sudo_password(password)
            .ssh(client)
            .run()?;
        let (_, cert_priv) = cmd!("cat", cert_priv_path)
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
            n if n <= 999 && n % 100 == 0 => format!("{}-hundred", Self::numeral(n / 100),).into(),
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
