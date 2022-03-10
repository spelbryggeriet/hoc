use std::{
    cell::{Ref, RefCell},
    fs,
    path::Path,
};

use osshkeys::{keys::FingerprintHash, KeyPair, PublicKey, PublicParts};
use structopt::StructOpt;

use hoclib::{attributes, ssh::SshClient, DirState};
use hoclog::{hidden_input, info, status, LogErr, Result};
use hocproc::procedure;

use crate::command::util::os::OperatingSystem;

use super::CreateUser;

procedure! {
    #[derive(StructOpt)]
    pub struct Configure {
        #[procedure(attribute)]
        #[structopt(long)]
        os: OperatingSystem,

        #[procedure(attribute)]
        #[structopt(long)]
        node_address: String,

        #[structopt(long)]
        username: String,

        #[structopt(skip)]
        password:  RefCell<Option<String>>,

        #[structopt(skip)]
        ssh_client: RefCell<Option<SshClient>>,
    }

    pub enum ConfigureState {
        Prepare,

        AddNewUser,
        AssignSudoPrivileges,
        DeletePiUser,
        SetUpSshAccess,
        #[procedure(finish)]
        InstallDependencies,

        #[procedure(finish)]
        ChangePassword,
    }
}

impl Run for ConfigureState {
    fn prepare(proc: &mut Configure, _work_dir_state: &mut DirState) -> Result<Self> {
        let state = match proc.os {
            OperatingSystem::RaspberryPiOs { .. } => AddNewUser,
            OperatingSystem::Ubuntu { .. } => ChangePassword,
        };

        Ok(state)
    }

    fn add_new_user(proc: &mut Configure, _work_dir_state: &mut DirState) -> Result<Self> {
        let username = &proc.username;
        let password = proc.password_for_user(username)?;
        let client = proc.ssh_client_password_auth(&proc.node_address, "pi", "raspberry")?;

        let priv_path = DirState::get_path::<CreateUser>(
            &attributes!("Username" => username),
            Path::new(&format!("ssh/id_{username}_ed25519")),
        )
        .log_context("user not found")?;
        let priv_key = fs::read_to_string(priv_path)?;

        KeyPair::from_keystr(&priv_key, Some(&password))
            .log_context("incorrect password provided")?;

        // Add the new user.
        adduser!(username)
            .stdin_lines([&*password, &*password])
            .sudo()
            .ssh(&client)
            .run()?;

        Ok(AssignSudoPrivileges)
    }

    fn assign_sudo_privileges(
        proc: &mut Configure,
        _work_dir_state: &mut DirState,
    ) -> Result<Self> {
        let username = &proc.username;
        let client = proc.ssh_client_password_auth(&proc.node_address, "pi", "raspberry")?;

        // Assign the user the relevant groups.
        usermod!(
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
        tee!(sudo_file)
            .stdin_line(&format!("{username} ALL=(ALL) PASSWD: ALL"))
            .sudo()
            .hide_output()
            .ssh(&client)
            .run()?;
        chmod!("440", sudo_file).sudo().ssh(&client).run()?;

        Ok(DeletePiUser)
    }

    fn delete_pi_user(proc: &mut Configure, _work_dir_state: &mut DirState) -> Result<Self> {
        let username = &proc.username;
        let password = proc.password_for_user(&username)?;
        let client = proc.ssh_client_password_auth(&proc.node_address, &username, &password)?;

        // Kill all processes owned by the `pi` user.
        pkill!("-u", "pi")
            .sudo_password(&*password)
            .success_codes([0, 1])
            .ssh(&client)
            .run()?;

        // Delete the default `pi` user.
        deluser!("--remove-home", "pi")
            .sudo_password(&*password)
            .ssh(&client)
            .run()?;

        Ok(SetUpSshAccess)
    }

    fn set_up_ssh_access(proc: &mut Configure, _work_dir_state: &mut DirState) -> Result<Self> {
        let username = &proc.username;
        let password = proc.password_for_user(username)?;

        let (pub_key, pub_path, priv_path) = status!("Read SSH keypair" => {
            let pub_path = DirState::get_path::<CreateUser>(
                &attributes!("Username" => username),
                Path::new(&format!("ssh/id_{username}_ed25519.pub")),
            )
            .log_context("user not found")?;
            let priv_path = DirState::get_path::<CreateUser>(
                &attributes!("Username" => username),
                Path::new(&format!("ssh/id_{username}_ed25519")),
            )
            .log_context("user not found")?;

            let pub_key = fs::read_to_string(&pub_path)?;
            info!(
                "SSH public key fingerprint randomart:\n{}",
                PublicKey::from_keystr(&pub_key)
                    .log_err()?
                    .fingerprint_randomart(FingerprintHash::SHA256)
                    .log_err()?
            );

            (pub_key, pub_path, priv_path)
        });

        let client = proc.ssh_client_password_auth(&proc.node_address, username, &password)?;

        status!("Send SSH public key" => {
            // Create the `.ssh` directory.
            mkdir!("-p", "-m", "700", format!("/home/{username}/.ssh"))
                .ssh(&client)
                .run()?;

            let dest = format!("/home/{username}/.ssh/authorized_keys");
            let src = dest.clone() + "_updated";

            // Check if the authorized keys file exists.
            let (status_code, _) = test!("-s", dest).success_codes([0, 1]).ssh(&client).run()?;
            if status_code == 1 {
                // Create the authorized keys file.
                cat!().stdin_line(username).stdout(&dest).ssh(&client).run()?;
                chmod!("644", dest).ssh(&client).run()?;
            }

            // Copy the public key to the authorized keys file.
            let key = pub_key.replace("/", r"\/");
            sed!(
                format!("0,/{username}$/{{h;s/^.*{username}$/{key}/}};${{x;/^$/{{s//{key}/;H}};x}}"),
                dest,
            )
            .stdout(&src)
            .secret(&key)
            .ssh(&client)
            .run()?;

            // Move the updated config contents.
            dd!(format!("if={src}"), format!("of={dest}")).ssh(&client).run()?;
            rm!(src).ssh(&client).run()?;
        });

        status!("Configure SSH server" => {
            let dest = "/etc/ssh/sshd_config";
            let src = format!("/home/{username}/sshd_config_updated");

            // Set `PasswordAuthentication` to `no`.
            let key = "PasswordAuthentication";
            sed!(
                format!("0,/{key}/{{h;s/^.*{key}.*$/{key} no/}};${{x;/^$/{{s//{key} no/;H}};x}}"),
                dest,
            )
            .stdout(&src)
            .ssh(&client)
            .run()?;

            // Move the updated config contents.
            dd!(format!("if={src}"), format!("of={dest}"))
                .sudo_password(&*password)
                .ssh(&client)
                .run()?;
            rm!(src).ssh(&client).run()?;

            // Verify sshd config and restart the SSH server.
            sshd!("-t").sudo_password(&*password).ssh(&client).run()?;
            systemctl!("restart", "ssh")
                .sudo_password(&*password)
                .ssh(&client)
                .run()?;

            // Verify again after SSH server restart.
            let client =
                proc.ssh_client_key_auth(&proc.node_address, username, &pub_path, &priv_path, &password)?;

            sshd!("-t").sudo_password(&*password).ssh(&client).run()?;
        });

        Ok(InstallDependencies)
    }

    fn install_dependencies(proc: &mut Configure, _work_dir_state: &mut DirState) -> Result<()> {
        let username = &proc.username;
        let pub_path = DirState::get_path::<CreateUser>(
            &attributes!("Username" => username),
            Path::new(&format!("ssh/id_{username}_ed25519.pub")),
        )
        .log_context("user not found")?;
        let priv_path = DirState::get_path::<CreateUser>(
            &attributes!("Username" => username),
            Path::new(&format!("ssh/id_{username}_ed25519")),
        )
        .log_context("user not found")?;

        let password = proc.password_for_user(&username)?;
        let _client = proc.ssh_client_key_auth(
            &proc.node_address,
            &username,
            &pub_path,
            &priv_path,
            &password,
        )?;

        hoclog::error!("not implemented")?;

        Ok(())
    }

    fn change_password(proc: &mut Configure, _work_dir_state: &mut DirState) -> Result<()> {
        let username = &proc.username;

        let pub_path = DirState::get_path::<CreateUser>(
            &attributes!("Username" => username),
            Path::new(&format!("ssh/id_{username}_ed25519.pub")),
        )
        .log_context("user not found")?;
        let priv_path = DirState::get_path::<CreateUser>(
            &attributes!("Username" => username),
            Path::new(&format!("ssh/id_{username}_ed25519")),
        )
        .log_context("user not found")?;

        let password = proc.password_for_user(username)?;
        let client = proc.ssh_client_key_auth(
            &proc.node_address,
            username,
            &pub_path,
            &priv_path,
            &password,
        )?;

        chpasswd!()
            .sudo_password(&"temporary_password")
            .stdin_line(&format!("{username}:{password}"))
            .ssh(&client)
            .run()?;

        Ok(())
    }
}

impl Configure {
    fn password_for_user(&self, username: &str) -> Result<Ref<String>> {
        if self.password.borrow().is_none() {
            let password = hidden_input!("Enter password for {}", username).get()?;
            self.password.replace(Some(password));
        }

        Ok(Ref::map(self.password.borrow(), |o| o.as_ref().unwrap()))
    }

    fn ssh_client_key_auth(
        &self,
        host: &str,
        username: &str,
        pub_key_path: &Path,
        priv_key_path: &Path,
        key_passphrase: &str,
    ) -> Result<Ref<SshClient>> {
        {
            let mut ref_mut = self.ssh_client.borrow_mut();
            if let Some(ref mut client) = *ref_mut {
                client.update_key_auth(username, pub_key_path, priv_key_path, key_passphrase)?;
            } else {
                let new_client = SshClient::new_key_auth(
                    host,
                    username,
                    pub_key_path,
                    priv_key_path,
                    key_passphrase,
                )?;
                ref_mut.replace(new_client);
            };
        }

        Ok(Ref::map(self.ssh_client.borrow(), |o| o.as_ref().unwrap()))
    }

    fn ssh_client_password_auth(
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
}