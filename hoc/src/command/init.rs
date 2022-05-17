use std::{
    cell::{Ref, RefCell},
    fs,
    net::IpAddr,
    path::PathBuf,
};

use osshkeys::{keys::FingerprintHash, PublicKey, PublicParts};
use serde::{Deserialize, Serialize};
use structopt::StructOpt;

use hoc_core::{
    cmd,
    kv::{self, ReadStore, WriteStore},
    ssh::SshClient,
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

        // Assign the user the relevant groups.
        cmd!(
            "usermod",
            "-a",
            "-G",
            "adm,dialout,cdrom,sudo,audio,video,plugdev,games,users,input,netdev,gpio,i2c,spi",
            username,
        )
        .sudo()
        .ssh(&client)
        .run()?;

        // Create sudo file for the user.
        let sudo_file = format!("/etc/sudoers.d/010_{username}");
        cmd!("tee", sudo_file)
            .stdin_line(&format!("{username} ALL=(ALL) PASSWD: ALL"))
            .sudo()
            .hide_output()
            .ssh(&client)
            .run()?;
        cmd!("chmod", "440", sudo_file).sudo().ssh(&client).run()?;

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

        // Kill all processes owned by the `pi` user.
        cmd!("pkill", "-u", "pi")
            .sudo_password(&*password)
            .success_codes([0, 1])
            .ssh(&client)
            .run()?;

        // Delete the default `pi` user.
        cmd!("deluser", "--remove-home", "pi")
            .sudo_password(&*password)
            .ssh(&client)
            .run()?;

        Ok(SetUpSshAccess)
    }

    fn set_up_ssh_access(proc: &mut Init, registry: &impl WriteStore) -> Result<Self> {
        let cluster = &proc.cluster;
        let username = &proc.username;
        let password = proc.get_password_for_user(username)?;

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

        let client = proc.get_ssh_client_with_password_auth(
            &proc.node_address.to_string(),
            username,
            &password,
        )?;

        status!("Send SSH public key").on(|| {
            // Create the `.ssh` directory.
            cmd!("mkdir", "-p", "-m", "700", format!("/home/{username}/.ssh"))
                .ssh(&client)
                .run()?;

            let dest = format!("/home/{username}/.ssh/authorized_keys");
            let src = dest.clone() + "_updated";

            // Check if the authorized keys file exists.
            let (status_code, _) = cmd!("test", "-s", dest)
                .success_codes([0, 1])
                .ssh(&client)
                .run()?;
            if status_code == 1 {
                // Create the authorized keys file.
                cmd!("cat")
                    .stdin_line(username)
                    .stdout(&dest)
                    .ssh(&client)
                    .run()?;
                cmd!("chmod", "644", dest).ssh(&client).run()?;
            }

            // Copy the public key to the authorized keys file.
            let key = pub_key.replace("/", r"\/");
            cmd!(
                "sed",
                format!(
                    "0,/{username}$/{{h;s/^.*{username}$/{key}/}};${{x;/^$/{{s//{key}/;H}};x}}"
                ),
                dest,
            )
            .stdout(&src)
            .secret(&key)
            .ssh(&client)
            .run()?;

            // Move the updated config contents.
            cmd!("dd", format!("if={src}"), format!("of={dest}"))
                .ssh(&client)
                .run()?;
            cmd!("rm", src).ssh(&client).run()?;

            hoc_log::Result::Ok(())
        })?;

        status!("Initialize SSH server").on(|| {
            let dest = "/etc/ssh/sshd_config";
            let src = format!("/home/{username}/sshd_config_updated");

            // Set `PasswordAuthentication` to `no`.
            let key = "PasswordAuthentication";
            cmd!(
                "sed",
                format!("0,/{key}/{{h;s/^.*{key}.*$/{key} no/}};${{x;/^$/{{s//{key} no/;H}};x}}"),
                dest,
            )
            .stdout(&src)
            .ssh(&client)
            .run()?;

            // Move the updated config contents.
            cmd!("dd", format!("if={src}"), format!("of={dest}"))
                .sudo_password(&*password)
                .ssh(&client)
                .run()?;
            cmd!("rm", src).ssh(&client).run()?;

            // Verify sshd config and restart the SSH server.
            cmd!("sshd", "-t")
                .sudo_password(&*password)
                .ssh(&client)
                .run()?;
            cmd!("systemctl", "restart", "ssh")
                .sudo_password(&*password)
                .ssh(&client)
                .run()?;

            // Verify again after SSH server restart.
            let client = proc.get_ssh_client_with_key_auth(registry, &password)?;

            cmd!("sshd", "-t")
                .sudo_password(&*password)
                .ssh(&client)
                .run()?;

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

        let nomad_filename = format!("{NOMAD_VERSION}.zip");
        let consul_filename = format!("{CONSUL_VERSION}.zip");
        let envoy_filename = format!("{ENVOY_VERSION}.tar.xz");

        // Nomad.
        cmd!("mkdir", "/run/nomad").ssh(&client).run()?;
        cmd!("wget", NOMAD_URL, "-O", nomad_filename)
            .working_directory("/run/nomad")
            .ssh(&client)
            .run()?;
        cmd!("unzip", "-o", nomad_filename, "-d", "/usr/local/bin")
            .working_directory("/run/nomad")
            .ssh(&client)
            .run()?;

        // Consul.
        cmd!("mkdir", "/run/consul").ssh(&client).run()?;
        cmd!("wget", CONSUL_URL, "-O", consul_filename)
            .working_directory("/run/consul")
            .ssh(&client)
            .run()?;
        cmd!("unzip", "-o", consul_filename, "-d", "/usr/local/bin")
            .working_directory("/run/consul")
            .ssh(&client)
            .run()?;

        // Envoy.
        cmd!("mkdir", "/run/envoy").ssh(&client).run()?;
        cmd!("wget", ENVOY_URL, "-O", envoy_filename)
            .working_directory("/run/envoy")
            .ssh(&client)
            .run()?;
        cmd!("xz", "-d", envoy_filename)
            .working_directory("/run/envoy")
            .ssh(&client)
            .run()?;
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
        .working_directory("/run/envoy")
        .ssh(&client)
        .run()?;

        Ok(InitializeNomad)
    }

    fn initialize_nomad(proc: &mut Init, registry: &impl WriteStore) -> Result<InitState> {
        let (_, client) = proc.get_password_and_ssh_client(registry)?;

        cmd!("nomad", "-autocomplete-install").ssh(&client).run()?;
        cmd!("complete", "-C", "/usr/local/bin/nomad", "nomad")
            .ssh(&client)
            .run()?;
        cmd!("systemctl", "daemon-reload").ssh(&client).run()?;
        cmd!("systemctl", "enable", "nomad").ssh(&client).run()?;
        cmd!("systemctl", "start", "nomad").ssh(&client).run()?;

        Ok(InitializeConsul)
    }

    fn initialize_consul(proc: &mut Init, registry: &impl WriteStore) -> Result<()> {
        let cluster = &proc.cluster;
        let (_, client) = proc.get_password_and_ssh_client(registry)?;

        cmd!("consul", "-autocomplete-install").ssh(&client).run()?;
        cmd!("complete", "-C", "/usr/local/bin/consul", "consul")
            .ssh(&client)
            .run()?;

        let registry_key = format!("clusters/{cluster}/key");
        let key: String = match registry.get(&registry_key) {
            Ok(key) => key.try_into()?,
            Err(kv::Error::KeyDoesNotExist(_)) => {
                let (_, key) = cmd!("consul", "keygen").ssh(&client).run()?;
                registry.put(&registry_key, key.clone())?;
                key
            }
            Err(err) => return Err(err.into()),
        };

        cmd!("consul", "tls", "ca", "create")
            .working_directory("/run/consul")
            .ssh(&client)
            .run()?;

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
