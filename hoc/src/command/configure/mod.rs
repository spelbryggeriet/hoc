use hoclog::{choose, hidden_input, input, status};
use hocproc::procedure;

use crate::{
    command::util::ssh,
    procedure::{Halt, ProcedureStep},
    Result,
};

mod util;

cmd_template! {
    adduser => "adduser", username;
    arp => "arp", "-a";
    usermod => "usermod", "-a", "-G", "adm,dialout,cdrom,sudo,audio,video,plugdev,games,users,input,netdev,gpio,i2c,spi", username;
}

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
    }
}

impl Configure {
    fn get_host(&self, _step: &mut ProcedureStep) -> Result<Halt<ConfigureState>> {
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

        Ok(Halt::persistent_yield(ConfigureState::AddNewUser {
            host: local_endpoint.host().into_owned(),
        }))
    }

    fn add_new_user(
        &mut self,
        _step: &mut ProcedureStep,
        host: String,
    ) -> Result<Halt<ConfigureState>> {
        let new_username = input!("Choose a new username");
        let new_password = hidden_input!("Choose a new password").verify().get()?;

        util::with_ssh_client(
            &mut self.ssh_client,
            util::Creds::default(&host),
            |client| {
                adduser!(new_username)
                    .ssh(&client)
                    .sudo()
                    .pipe_input([new_password.clone(), new_password])
                    .run()?;
                Ok(())
            },
        )?;

        Ok(Halt::persistent_yield(ConfigureState::AddGroupsToNewUser {
            host,
            new_username,
        }))
    }

    fn add_groups_to_new_user(
        &mut self,
        _step: &mut ProcedureStep,
        host: String,
        new_username: String,
    ) -> Result<Halt<ConfigureState>> {
        util::with_ssh_client(
            &mut self.ssh_client,
            util::Creds::default(&host),
            |client| {
                usermod!(new_username).ssh(&client).sudo().run()?;
                Ok(())
            },
        )?;

        Ok(Halt::persistent_finish())
    }
}
