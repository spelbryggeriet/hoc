use std::{
    cell::{Ref, RefCell},
    fs,
};

use hoclib::{cmd_macros, finish, halt, ssh::SshClient, Halt, ProcedureStep};
use hoclog::{choose, hidden_input, info, input, status, LogErr, Result};
use hocproc::procedure;
use osshkeys::{cipher::Cipher, keys::FingerprintHash, KeyPair, KeyType};

cmd_macros!(
    adduser, arp, cat, chmod, dd, deluser, mkdir, pkill, rm, sed, sshd, systemctl, tee, test,
    usermod,
);

mod util;

procedure! {
    pub struct Configure {
        #[procedure(attribute)]
        node_name: String,

        #[structopt(skip)]
        password:  RefCell<Option<String>>,

        #[structopt(skip)]
        ssh_client: RefCell<Option<SshClient>>,
    }

    pub enum ConfigureState {
        GetHost,
        AddNewUser { host: String },
        AssignSudoPrivileges { host: String, username: String },
        DeletePiUser { host: String, username: String },
        SetUpSshAccess { host: String, username: String },
    }
}

impl Steps for Configure {
    fn get_host(&mut self, _step: &mut ProcedureStep) -> Result<Halt<ConfigureState>> {
        let local_endpoint = status!("Finding local endpoints" => {
            let (_, output) = arp!("-a").hide_stdout().run()?;
            let (default_index, mut endpoints) = util::LocalEndpoint::parse_arp_output(&output, &self.node_name);

            let index = choose!(
                "Which endpoint do you want to configure?",
                items = &endpoints,
                default_index = default_index,
            )?;

            endpoints.remove(index)
        });

        halt!(AddNewUser {
            host: local_endpoint.host().into_owned()
        })
    }

    fn add_new_user(
        &mut self,
        _step: &mut ProcedureStep,
        host: String,
    ) -> Result<Halt<ConfigureState>> {
        let new_username = input!("Choose a new username");
        let new_password = hidden_input!("Choose a new password").verify().get()?;

        let client = self.ssh_client_password_auth(&host, "pi", "raspberry")?;

        // Add the new user.
        adduser!(new_username)
            .stdin_lines([&new_password, &new_password])
            .sudo()
            .ssh(&client)
            .run()?;

        self.password.replace(Some(new_password));

        halt!(AssignSudoPrivileges {
            host,
            username: new_username
        })
    }

    fn assign_sudo_privileges(
        &mut self,
        _step: &mut ProcedureStep,
        host: String,
        username: String,
    ) -> Result<Halt<ConfigureState>> {
        let client = self.ssh_client_password_auth(&host, "pi", "raspberry")?;

        // Assign the user the relevant groups.
        usermod!(
            "-a",
            "-G",
            "adm,dialout,cdrom,sudo,audio,video,plugdev,games,users,input,netdev,gpio,i2c,spi",
            username
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

        halt!(DeletePiUser { host, username })
    }

    fn delete_pi_user(
        &mut self,
        _step: &mut ProcedureStep,
        host: String,
        username: String,
    ) -> Result<Halt<ConfigureState>> {
        let password = self.password_for_user(&username)?;
        let client = self.ssh_client_password_auth(&host, &username, &password)?;

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

        halt!(SetUpSshAccess { host, username })
    }

    fn set_up_ssh_access(
        &mut self,
        step: &mut ProcedureStep,
        host: String,
        username: String,
    ) -> Result<Halt<ConfigureState>> {
        let password = self.password_for_user(&username)?;

        let (pub_key, priv_key) = status!("Generating SSH keypair" => {
            let mut key_pair = KeyPair::generate(KeyType::ED25519, 256).log_err()?;
            *key_pair.comment_mut() = username.clone();

            let pub_key = key_pair.serialize_publickey().log_err()?;
            let priv_key = key_pair.serialize_openssh(Some(&password), Cipher::Aes256_Cbc).log_err()?;

            let randomart = util::fingerprint_randomart(
                FingerprintHash::SHA256,
                &key_pair,
            )?;

            info!("Fingerprint randomart:");
            info!(randomart);

            (pub_key, priv_key)
        });

        status!("Storing SSH keypair" => {
            let pub_path = step.register_file(format!("ssh/id_{username}_ed25519.pub"))?;
            let priv_path = step.register_file(format!("ssh/id_{username}_ed25519"))?;
            fs::write(pub_path, &pub_key)?;
            fs::write(&priv_path, priv_key)?;

            info!("Key stored in {}", priv_path.to_string_lossy());
        });

        let client = self.ssh_client_password_auth(&host, &username, &password)?;

        status!("Sending SSH public key" => {
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
                cat!().stdin_line(&username).stdout(&dest).ssh(&client).run()?;
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

        status!("Configuring SSH server" => {
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
            hoclog::error!("Abort")?;
            systemctl!("restart", "ssh")
                .sudo_password(&*password)
                .ssh(&client)
                .run()?;
        });

        finish!()
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
