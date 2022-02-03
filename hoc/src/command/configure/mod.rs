use std::net::Ipv4Addr;

use hoclog::{choose, hidden_input, status};
use serde::{Deserialize, Serialize};
use structopt::StructOpt;
use strum::{EnumDiscriminants, EnumString, IntoStaticStr};

use crate::{
    command::util::ssh::Client,
    procedure::{Attributes, Halt, Procedure, ProcedureState, ProcedureStateId, ProcedureStep},
    Result,
};

mod change_default_user;
mod get_host;

#[derive(StructOpt)]
pub struct Configure {
    node_name: String,

    #[structopt(skip)]
    ssh_client: Option<Client>,
}

#[derive(Debug, Serialize, Deserialize, EnumDiscriminants)]
#[strum_discriminants(derive(Hash, PartialOrd, Ord, EnumString, IntoStaticStr))]
#[strum_discriminants(name(ConfigureStateId))]
pub enum ConfigureState {
    GetHost,
    ChangeDefaultUser { host: String },
}

impl Procedure for Configure {
    type State = ConfigureState;
    const NAME: &'static str = "configure";

    fn get_attributes(&self) -> Attributes {
        let mut variant = Attributes::new();
        variant.insert("Node name".to_string(), self.node_name.clone().into());
        variant
    }

    fn run(&mut self, step: &mut ProcedureStep) -> Result<Halt<ConfigureState>> {
        let halt = match step.state()? {
            ConfigureState::GetHost => self.get_host(step)?,
            ConfigureState::ChangeDefaultUser { host } => self.change_default_user(step, host)?,
        };

        Ok(halt)
    }
}

impl ProcedureStateId for ConfigureStateId {
    type DeserializeError = strum::ParseError;

    fn description(&self) -> &'static str {
        match self {
            Self::GetHost => "Get host",
            Self::ChangeDefaultUser => "Change default user",
        }
    }
}

impl Default for ConfigureState {
    fn default() -> Self {
        ConfigureState::GetHost
    }
}

impl ProcedureState for ConfigureState {
    type Id = ConfigureStateId;

    fn id(&self) -> Self::Id {
        self.into()
    }
}
