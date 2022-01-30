use super::*;
use crate::command::util::ssh::{Authentication, Client};

impl Configure {
    pub(super) fn change_default_user(
        &mut self,
        step: &mut ProcedureStep,
        host: String,
    ) -> Result<Halt<ConfigureState>> {
        let new_password = hidden_input!("Choose a new password");

        status!("Connecting to node '{}' at host '{}'", self.node_name, host => {
            let client = Client::new(host, "pi", Authentication::Password("raspberry"))?;

            cmd!("ls", "-la").ssh(&client).run()?;

            self.ssh_client = Some(client);
        });

        Ok(Halt::persistent_finish())
    }
}
