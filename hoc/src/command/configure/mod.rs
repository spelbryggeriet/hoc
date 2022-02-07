use colored::Colorize;
use hoclog::{choose, hidden_input, input, status, LogErr, Result};
use hocproc::procedure;

use hoclib::{cmd_template, finish, halt, ssh, Halt, ProcedureStep};

cmd_template! {
    adduser => "adduser", username;
    arp => "arp", "-a";
    tee => "tee", file;
    usermod => "usermod", "-a", "-G", "adm,dialout,cdrom,sudo,audio,video,plugdev,games,users,input,netdev,gpio,i2c,spi", username;
}

mod util;

procedure! {
    pub struct Configure {
        #[procedure(attribute)]
        node_name: String,

        #[structopt(skip)]
        ssh_client: Option<ssh::Client>,
    }

    pub enum ConfigureState {
        GetHost,
        AddNewUser { host: String },
        AddGroupsToNewUser { host: String, new_username: String },
        AddSudoPasswordRequirement { host: String, new_username: String },
    }
}

impl Steps for Configure {
    fn get_host(&mut self, _step: &mut ProcedureStep) -> Result<Halt<ConfigureState>> {
        let local_endpoint = status!("Finding local endpoints" => {
            let output = arp!().hide_output().run().log_err()?;
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

        self.with_ssh_client(util::Creds::default(&host), |client| {
            adduser!(new_username)
                .ssh(&client)
                .sudo()
                .pipe_input([new_password.clone(), new_password])
                .run()
                .log_err()
        })
        .log_err()?;

        halt!(AddGroupsToNewUser { host, new_username })
    }

    fn add_groups_to_new_user(
        &mut self,
        _step: &mut ProcedureStep,
        host: String,
        new_username: String,
    ) -> Result<Halt<ConfigureState>> {
        self.with_ssh_client(util::Creds::default(&host), |client| {
            usermod!(new_username).ssh(&client).sudo().run().log_err()
        })
        .log_err()?;

        halt!(AddSudoPasswordRequirement { host, new_username })
    }

    fn add_sudo_password_requirement(
        &mut self,
        _step: &mut ProcedureStep,
        host: String,
        new_username: String,
    ) -> Result<Halt<ConfigureState>> {
        self.with_ssh_client(util::Creds::default(&host), |client| {
            tee!("/etc/sudoers.d/010_pi-nopasswd")
                .ssh(&client)
                .sudo()
                .pipe_input([format!("{new_username} ALL=(ALL) PASSWD: ALL")])
                .hide_output()
                .run()
                .log_err()
        })
        .log_err()?;

        finish!()
    }
}

impl Configure {
    pub fn with_ssh_client<T>(
        &mut self,
        creds: util::Creds,
        f: impl FnOnce(&ssh::Client) -> Result<T>,
    ) -> Result<T> {
        if let Some(ref client) = self.ssh_client {
            f(client)
        } else {
            let new_client = status!("Connecting to host {}", creds.host.blue() => {
                 ssh::Client::new(creds.host.to_string(), creds.username, creds.auth).log_err()?
            });

            let output = f(&new_client)?;
            self.ssh_client.replace(new_client);
            Ok(output)
        }
    }
}