use std::cell::{Ref, RefCell};

use hoclog::{choose, hidden_input, input, status, Result};
use hocproc::procedure;

use hoclib::{cmd_template, finish, halt, ssh::SshClient, Halt, ProcedureStep, ProcessError};

cmd_template! {
    adduser(username) => "adduser", username;
    arp() => "arp", "-a";
    chmod(file) => "chmod", "0440", file;
    deluser() => "deluser", "--remove-home", "pi";
    pkill() => "pkill", "-u", "pi";
    tee(file) => "tee", file;
    usermod(username) => "usermod", "-a", "-G", "adm,dialout,cdrom,sudo,audio,video,plugdev,games,users,input,netdev,gpio,i2c,spi", username;
}

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
    }
}

impl Steps for Configure {
    fn get_host(&mut self, _step: &mut ProcedureStep) -> Result<Halt<ConfigureState>> {
        let local_endpoint = status!("Finding local endpoints" => {
            let output = arp!().hide_output().run()?;
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
            .pipe_input([&new_password, &new_password])
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

        usermod!(username).ssh(&client).sudo().run()?;

        let sudo_file = format!("/etc/sudoers.d/010_{username}");

        tee!(sudo_file)
            .ssh(&client)
            .sudo()
            .pipe_input([&format!("{username} ALL=(ALL) PASSWD: ALL")])
            .hide_output()
            .run()?;

        chmod!(sudo_file).ssh(&client).sudo().run()?;

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

        let result = pkill!().ssh(&client).sudo_password(&*password).run();
        match result {
            Ok(_) | Err(ProcessError::Exit { status: 1, .. }) => (),
            Err(err) => return Err(err.into()),
        }

        deluser!().ssh(&client).sudo_password(&*password).run()?;

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
