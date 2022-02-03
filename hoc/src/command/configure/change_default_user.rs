use colored::Colorize;
use hoclog::input;

use super::*;
use crate::command::util::ssh::{Authentication, Client};

impl Configure {
    pub(super) fn change_default_user(
        &mut self,
        _step: &mut ProcedureStep,
        host: String,
    ) -> Result<Halt<ConfigureState>> {
        let new_username = input!("Choose a new username");
        let new_password = hidden_input!("Choose a new password").verify().get()?;

        status!("Connecting to node {} at host {}", self.node_name.blue(), host.blue() => {
            let client = Client::new(host, "pi", Authentication::Password("raspberry"))?;

            cmd!("adduser", new_username)
                .ssh(&client)
                .sudo()
                .pipe_input([new_password.clone(), new_password])
                .run()?;

            cmd!(
                "usermod",
                "-a",
                "-G",
                "adm,dialout,cdrom,sudo,audio,video,plugdev,games,users,input,netdev,gpio,i2c,spi",
                new_username,
            )
            .ssh(&client)
            .sudo()
            .run()?;

            self.ssh_client = Some(client);
        });

        Ok(Halt::persistent_finish())
    }
}
