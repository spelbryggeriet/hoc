use std::{
    cell::{Ref, RefCell},
    fs::{self, File},
    io::{Read, Write},
    net::IpAddr,
    path::PathBuf,
};

use osshkeys::{keys::FingerprintHash, PublicKey, PublicParts};
use serde::{Deserialize, Serialize};
use structopt::StructOpt;

use hoc_core::{
    cmd,
    kv::{self, ReadStore, WriteStore},
    process::{self, ssh::SshClient},
};
use hoc_log::{hidden_input, info, status, LogErr, Result};
use hoc_macros::{Procedure, ProcedureState};

use crate::command::util::os::OperatingSystem;

const NOMAD_URL: &str = "https://releases.hashicorp.com/nomad/1.2.6/nomad_1.2.6_linux_arm64.zip";
const NOMAD_VERSION: &str = "1.2.6";

const CONSUL_URL: &str =
    "https://releases.hashicorp.com/consul/1.11.4/consul_1.11.4_linux_arm64.zip";
const CONSUL_VERSION: &str = "1.11.4";

const ENVOY_URL: &str =
    "https://archive.tetratelabs.io/envoy/download/v1.21.1/envoy-v1.20.2-linux-arm64.tar.xz";
const ENVOY_VERSION: &str = "1.20.2";

#[derive(Procedure, StructOpt)]
#[procedure(dependencies(PrepareSdCard(cluster=cluster, nodeName=node_name)))]
pub struct Init {
    #[procedure(attribute)]
    #[structopt(long)]
    cluster: String,

    #[procedure(attribute)]
    #[structopt(long)]
    node_name: String,

    #[structopt(long)]
    node_os: OperatingSystem,

    #[structopt(long)]
    node_address: IpAddr,

    #[structopt(long)]
    username: String,

    #[structopt(skip)]
    password: RefCell<Option<String>>,

    #[structopt(skip)]
    ssh_client: RefCell<Option<SshClient>>,
}

#[derive(ProcedureState, Serialize, Deserialize)]
pub enum InitState {
    Prepare,

    AddNewUser,
    AssignSudoPrivileges,
    DeletePiUser,
    SetUpSshAccess,
    ChangePassword,

    InstallDependencies,
    InitializeNomad,
    #[state(finish)]
    InitializeConsul,
}

impl Run for InitState {
    fn prepare(proc: &mut Init, _registry: &impl WriteStore) -> Result<Self> {
        let state = match proc.node_os {
            OperatingSystem::RaspberryPiOs { .. } => AddNewUser,
            OperatingSystem::Ubuntu { .. } => ChangePassword,
        };

        Ok(state)
    }

    fn add_new_user(proc: &mut Init, _registry: &impl WriteStore) -> Result<Self> {
        let username = &proc.username;
        let password = proc.get_password_for_user(username)?;
        let client = proc.get_ssh_client_with_password_auth(
            &proc.node_address.to_string(),
            "pi",
            "raspberry",
        )?;

        // Add the new user.
        cmd!("adduser", username)
            .stdin_lines([&*password, &*password])
            .sudo()
            .ssh(&client)
            .run()?;

        Ok(AssignSudoPrivileges)
    }

    fn assign_sudo_privileges(proc: &mut Init, _registry: &impl WriteStore) -> Result<Self> {
        let username = &proc.username;
        let client = proc.get_ssh_client_with_password_auth(
            &proc.node_address.to_string(),
            "pi",
            "raspberry",
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

    fn delete_pi_user(proc: &mut Init, _registry: &impl WriteStore) -> Result<Self> {
        let username = &proc.username;
        let password = proc.get_password_for_user(&username)?;
        let client = proc.get_ssh_client_with_password_auth(
            &proc.node_address.to_string(),
            &username,
            &password,
        )?;
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

    fn set_up_ssh_access(proc: &mut Init, registry: &impl WriteStore) -> Result<Self> {
        let cluster = &proc.cluster;
        let username = &proc.username;
        let password = proc.get_password_for_user(username)?;
        let client = proc.get_ssh_client_with_password_auth(
            &proc.node_address.to_string(),
            username,
            &password,
        )?;
        let common_set = process::Settings::default().ssh(&client);
        let sudo_set = process::Settings::from_settings(&common_set).sudo_password(&*password);

        let pub_key = status!("Read SSH keypair").on(|| {
            let pub_path: PathBuf = registry
                .get(format!("clusters/{cluster}/users/{username}/ssh/pub"))?
                .try_into()?;

            let pub_key = fs::read_to_string(&pub_path)?;
            info!(
                "SSH public key fingerprint randomart:\n{}",
                PublicKey::from_keystr(&pub_key)
                    .log_err()?
                    .fingerprint_randomart(FingerprintHash::SHA256)
                    .log_err()?
            );

            hoc_log::Result::Ok(pub_key)
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
                cmd!("cat")
                    .settings(&common_set)
                    .stdin_line(username)
                    .stdout(&authorized_keys_path)
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

            hoc_log::Result::Ok(())
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
            let client = proc.get_ssh_client_with_key_auth(registry, &password)?;

            cmd!("sshd", "-t").settings(&sudo_set).ssh(&client).run()?;

            hoc_log::Result::Ok(())
        })?;

        Ok(InstallDependencies)
    }

    fn change_password(proc: &mut Init, registry: &impl WriteStore) -> Result<InitState> {
        let username = &proc.username;
        let (password, client) = proc.get_password_and_ssh_client(registry)?;

        cmd!("chpasswd")
            .sudo_password("temporary_password")
            .stdin_line(format!("{username}:{password}"))
            .ssh(&client)
            .run()?;

        Ok(InstallDependencies)
    }

    fn install_dependencies(proc: &mut Init, registry: &impl WriteStore) -> Result<InitState> {
        let (_, client) = proc.get_password_and_ssh_client(registry)?;
        let common_set = process::Settings::default().ssh(&client);
        let all_set =
            process::Settings::from_settings(&common_set).working_directory("/run/consul");

        let nomad_filename = format!("{NOMAD_VERSION}.zip");
        let consul_filename = format!("{CONSUL_VERSION}.zip");
        let envoy_filename = format!("{ENVOY_VERSION}.tar.xz");

        // Nomad.
        cmd!("mkdir", "/run/nomad").run_with(&common_set)?;
        cmd!("wget", NOMAD_URL, "-O", nomad_filename).run_with(&all_set)?;
        cmd!("unzip", "-o", nomad_filename, "-d", "/usr/local/bin").run_with(&all_set)?;

        // Consul.
        cmd!("mkdir", "/run/consul").run_with(&common_set)?;
        cmd!("wget", CONSUL_URL, "-O", consul_filename).run_with(&all_set)?;
        cmd!("unzip", "-o", consul_filename, "-d", "/usr/local/bin").run_with(&all_set)?;

        // Envoy.
        cmd!("mkdir", "/run/envoy").run_with(&common_set)?;
        cmd!("wget", ENVOY_URL, "-O", envoy_filename).run_with(&all_set)?;
        cmd!("xz", "-d", envoy_filename).run_with(&all_set)?;
        cmd!(
            "tar",
            "-xf",
            envoy_filename.trim_end_matches(".xz"),
            "--overwrite",
            "--strip-components",
            "2",
            "-C",
            "/usr/local/bin"
        )
        .run_with(&all_set)?;

        Ok(InitializeNomad)
    }

    fn initialize_nomad(proc: &mut Init, registry: &impl WriteStore) -> Result<InitState> {
        let (_, client) = proc.get_password_and_ssh_client(registry)?;
        let common_set = process::Settings::default().ssh(&client);

        cmd!("nomad", "-autocomplete-install").run_with(&common_set)?;
        cmd!("complete", "-C", "/usr/local/bin/nomad", "nomad").run_with(&common_set)?;
        cmd!("systemctl", "enable", "nomad").run_with(&common_set)?;
        cmd!("systemctl", "start", "nomad").run_with(&common_set)?;

        Ok(InitializeConsul)
    }

    fn initialize_consul(proc: &mut Init, registry: &impl WriteStore) -> Result<()> {
        let cluster = &proc.cluster;
        let node_address = &proc.node_address;
        let (password, client) = proc.get_password_and_ssh_client(registry)?;
        let common_set = process::Settings::default().ssh(&client);

        // Set up command autocomplete.
        cmd!("consul", "-autocomplete-install").run_with(&common_set)?;
        cmd!("complete", "-C", "/usr/local/bin/consul", "consul").run_with(&common_set)?;

        // Generate common cluster key.
        let registry_key = format!("clusters/{cluster}/key");
        let encrypt_key: String = match registry.get(&registry_key).and_then(String::try_from) {
            Ok(key) => key,
            Err(kv::Error::KeyDoesNotExist(_)) => {
                let (_, key) = cmd!("consul", "keygen").run_with(&common_set)?;
                registry.put(&registry_key, key.clone())?;
                key
            }
            Err(err) => return Err(err.into()),
        };

        cmd!("mkdir", "-p", "/etc/consul.d/certs").run_with(&common_set)?;

        // Create or distribute certificate authority certificate.
        let cert_pub_key = format!("clusters/{cluster}/certs/ca_pub");
        let cert_priv_key = format!("clusters/{cluster}/certs/ca_priv");
        let cert_pub_path = "/etc/consul.d/certs/consul-agent-ca.pem";
        let cert_priv_path = "/etc/consul.d/certs/consul-agent-ca-key.pem";
        match registry.get(&cert_pub_key).and_then(kv::FileRef::try_from) {
            Ok(ca_pub_file_ref) => {
                // Read certificate and key files.
                let ca_priv_file_ref: kv::FileRef = registry.get(cert_priv_key)?.try_into()?;
                let mut ca_pub_file = File::open(ca_pub_file_ref.path())?;
                let mut ca_priv_file = File::open(ca_priv_file_ref.path())?;
                let mut ca_pub = String::new();
                let mut ca_priv = String::new();
                ca_pub_file.read_to_string(&mut ca_pub)?;
                ca_priv_file.read_to_string(&mut ca_priv)?;

                // Send certificate and key to server.
                cmd!("cat")
                    .settings(&common_set)
                    .stdin_lines(ca_pub.lines())
                    .stdout(&cert_pub_path)
                    .run()?;
                cmd!("cat")
                    .settings(&common_set)
                    .stdin_lines(ca_priv.lines())
                    .stdout(&cert_priv_path)
                    .run()?;
            }
            Err(kv::Error::KeyDoesNotExist(_)) => {
                // Create CA certificate.
                cmd!("consul", "tls", "ca", "create")
                    .settings(&common_set)
                    .working_directory("/etc/consul.d/certs")
                    .run()?;

                // Download certificate and key.
                let (_, ca_pub) = cmd!("cat", cert_pub_path).run_with(&common_set)?;
                let (_, ca_priv) = cmd!("cat", cert_priv_path)
                    .settings(&common_set)
                    .hide_stdout()
                    .run()?;

                // Store certificate and key in registry.
                let ca_pub_file_ref = registry.create_file(cert_pub_key)?;
                let ca_priv_file_ref = registry.create_file(cert_priv_key)?;
                let mut ca_pub_file = File::options()
                    .create_new(true)
                    .write(true)
                    .open(ca_pub_file_ref.path())?;
                let mut ca_priv_file = File::options()
                    .create_new(true)
                    .write(true)
                    .open(ca_priv_file_ref.path())?;
                ca_pub_file.write_all(ca_pub.as_bytes())?;
                ca_priv_file.write_all(ca_priv.as_bytes())?;
            }
            Err(err) => return Err(err.into()),
        }

        // Create server certificates.
        cmd!("consul", "tls", "cert", "create", "-server", "-dc", cluster, "-domain", "consul")
            .settings(&common_set)
            .working_directory("/etc/consul.d/certs")
            .run()?;

        // Remove key from server.
        cmd!("rm", cert_priv_path).run_with(&common_set)?;

        // Set or get auto-join address.
        let auto_join_key = "clusters/{cluster}/auto_join_address";
        let auto_join_address = match registry.get(&auto_join_key).and_then(String::try_from) {
            Ok(address) => address,
            Err(kv::Error::KeyDoesNotExist(_)) => {
                let address = node_address.to_string();
                registry.put(auto_join_key, address.clone())?;
                address
            }
            Err(err) => return Err(err.into()),
        };

        // Update Consul configuration file
        let encrypt_sed = format!(r#"s/^(encrypt = \[")temporary_key("\])$/\1{encrypt_key}\2/"#);
        let retry_join_sed =
            format!(r#"s/^(retry_join = \[")temporary_password("\])$/\1{auto_join_address}\2/"#);
        let consul_config_path = "/etc/consul.d/consul.hcl";
        cmd!("sed", "-ri", encrypt_sed, consul_config_path).run_with(&common_set)?;
        cmd!("sed", "-ri", retry_join_sed, consul_config_path).run_with(&common_set)?;

        // Validate Consul configuration.
        cmd!("consul", "validate", "/etc/consul.d/")
            .settings(&common_set)
            .sudo_password(&*password)
            .run()?;

        // Start Consul service.
        cmd!("systemctl", "enable", "consul").run_with(&common_set)?;
        cmd!("systemctl", "start", "consul").run_with(&common_set)?;

        Ok(())
    }
}

impl Init {
    fn get_password_for_user(&self, username: &str) -> Result<Ref<String>> {
        if self.password.borrow().is_none() {
            let password = hidden_input!("Enter password for {}", username).get()?;
            self.password.replace(Some(password));
        }

        Ok(Ref::map(self.password.borrow(), |o| o.as_ref().unwrap()))
    }

    fn get_ssh_client_with_password_auth(
        &self,
        host: &str,
        username: &str,
        password: &str,
    ) -> Result<Ref<SshClient>> {
        {
            let mut ref_mut = self.ssh_client.borrow_mut();
            if let Some(ref mut client) = *ref_mut {
                client.update_password_auth(username, password)?;
            } else {
                let new_client = SshClient::new_password_auth(host, username, password)?;
                ref_mut.replace(new_client);
            };
        }

        Ok(Ref::map(self.ssh_client.borrow(), |o| o.as_ref().unwrap()))
    }

    fn get_ssh_client_with_key_auth(
        &self,
        registry: &impl ReadStore,
        key_passphrase: &str,
    ) -> Result<Ref<SshClient>> {
        let host = &self.node_address.to_string();
        let cluster = &self.cluster;
        let username = &self.username;

        let pub_path: PathBuf = registry
            .get(format!("clusters/{cluster}/users/{username}/ssh/pub"))?
            .try_into()?;
        let priv_path: PathBuf = registry
            .get(format!("clusters/{cluster}/users/{username}/ssh/priv"))?
            .try_into()?;

        {
            let mut ref_mut = self.ssh_client.borrow_mut();
            if let Some(ref mut client) = *ref_mut {
                client.update_key_auth(username, pub_path, priv_path, key_passphrase)?;
            } else {
                let new_client =
                    SshClient::new_key_auth(host, username, pub_path, priv_path, key_passphrase)?;
                ref_mut.replace(new_client);
            };
        }

        Ok(Ref::map(self.ssh_client.borrow(), |o| o.as_ref().unwrap()))
    }

    fn get_password_and_ssh_client<'a>(
        &self,
        registry: &impl ReadStore,
    ) -> Result<(Ref<String>, Ref<SshClient>)> {
        let username = &self.username;
        let password = self.get_password_for_user(username)?;
        let client = self.get_ssh_client_with_key_auth(registry, &password)?;

        Ok((password, client))
    }
}
