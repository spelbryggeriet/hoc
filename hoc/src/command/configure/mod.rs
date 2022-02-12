use std::{
    cell::{Ref, RefCell},
    fs,
};

use hoclog::{choose, hidden_input, info, input, status, LogErr, Result};
use hocproc::procedure;

use hoclib::{cmd_macros, finish, halt, ssh::SshClient, Halt, ProcedureStep};
use osshkeys::{cipher::Cipher, keys::FingerprintHash, KeyPair, KeyType};

cmd_macros!(adduser, arp, chmod, deluser, mkdir, mv, pkill, sed, systemctl, tee, test, usermod);

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

        adduser!(new_username)
            .ssh(&client)
            .sudo()
            .stdin_lines([&new_password, &new_password])
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

        usermod!(
            "-a",
            "-G",
            "adm,dialout,cdrom,sudo,audio,video,plugdev,games,users,input,netdev,gpio,i2c,spi",
            username
        )
        .ssh(&client)
        .sudo()
        .run()?;

        let sudo_file = format!("/etc/sudoers.d/010_{username}");

        tee!(sudo_file)
            .ssh(&client)
            .sudo()
            .stdin_line(&format!("{username} ALL=(ALL) PASSWD: ALL"))
            .hide_output()
            .run()?;

        chmod!("0440", sudo_file).ssh(&client).sudo().run()?;

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

        pkill!("-u", "pi")
            .ssh(&client)
            .sudo_password(&*password)
            .success_codes([0, 1])
            .run()?;

        deluser!("--remove-home", "pi")
            .ssh(&client)
            .sudo_password(&*password)
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
            let src = format!("/home/{username}/.ssh/authorized_keys");
            let dest = src.clone() + " updated";

            let (status_code, _) = test!("-s", src).ssh(&client).success_codes([0, 1]).run()?;
            if status_code == 1 {
                tee!(src).ssh(&client).stdin_line(&username).run()?;
            }

            let key = pub_key.replace("/", r"\/");
            sed!(
                format!("/{username}$/{{h;s/.*/{key}/}};${{x;/^$/{{s//{key}/;H}};x}}"),
                src,
            )
            .ssh(&client)
            .stdout(&dest)
            .secret(&key)
            .run()?;

            mv!(dest, src).ssh(&client).run()?;
        });

        hoclog::error!("Abort")?;

        status!("Configuring SSH server" => {
            mkdir!("-p", "-m", "700", format!("/home/{username}/.ssh"))
                .ssh(&client)
                .run()?;

            tee!("/etc/ssh/sshd_config")
                .ssh(&client)
                .sudo_password(&*password)
                .stdin_lines(&[
                    "Include /etc/ssh/sshd_config.d/*.conf",
                    "ChallengeResponseAuthentication no",
                    "UsePAM no",
                    "PasswordAuthentication no",
                    "PrintMotd no",
                    "AcceptEnv LANG LC_*",
                    "Subsystem sftp /usr/lib/openssh/sftp-server",
                ])
                .run()?;

            mkdir!("-p", "-m", "755", "/etc/ssh/sshd_config.d")
                .ssh(&client)
                .sudo_password(&*password)
                .run()?;

            let sshd_config_path = format!("/etc/ssh/sshd_config.d/010_{username}-allowusers.conf");

            tee!(sshd_config_path)
                .ssh(&client)
                .sudo_password(&*password)
                .stdin_line(&format!("AllowUsers {username}"))
                .run()?;

            //systemctl!("restart", "ssh")
            //.ssh(&client)
            //.sudo_password(&*password)
            //.run()?;
        });

        hoclog::error!("Abort")?;
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
