use std::{borrow::Cow, collections::HashMap};

use serde::{Deserialize, Serialize};
use structopt::StructOpt;
use strum::{EnumDiscriminants, EnumString, IntoStaticStr};

use crate::{
    procedure::{Attributes, Halt, Procedure, ProcedureState, ProcedureStateId, ProcedureStep},
    Result,
};
use hoclog::{bail, choose, info, prompt, status, LogErr};

use self::local_endpoints::LocalEndpoint;

mod local_endpoints;
mod node_settings;

#[derive(StructOpt)]
pub struct Configure {
    node_name: String,
}

#[derive(Debug, Serialize, Deserialize, EnumDiscriminants)]
#[strum_discriminants(derive(Hash, PartialOrd, Ord, EnumString, IntoStaticStr))]
#[strum_discriminants(name(ConfigureStateId))]
pub enum ConfigureState {
    LocalEndpoints,
    NodeSettings { local_endpoint: LocalEndpoint },
}

impl Procedure for Configure {
    type State = ConfigureState;
    const NAME: &'static str = "configure";

    fn rewind_state(&self) -> Option<ConfigureStateId> {
        Some(ConfigureStateId::LocalEndpoints)
    }

    fn get_attributes(&self) -> Attributes {
        let mut variant = Attributes::new();
        variant.insert("Node name".to_string(), self.node_name.clone().into());
        variant
    }

    fn run(&mut self, step: &mut ProcedureStep) -> Result<Halt<ConfigureState>> {
        let halt = match step.state()? {
            ConfigureState::LocalEndpoints => self.get_local_endpoints(step)?,
            ConfigureState::NodeSettings { local_endpoint } => {
                self.configure_node_settings(step, local_endpoint)?
            }
        };

        Ok(halt)
    }
}

impl ProcedureStateId for ConfigureStateId {
    type DeserializeError = strum::ParseError;

    fn description(&self) -> &'static str {
        match self {
            Self::LocalEndpoints => "Get local endpoints",
            Self::NodeSettings => "Configure node settings",
        }
    }
}

impl Default for ConfigureState {
    fn default() -> Self {
        ConfigureState::LocalEndpoints
    }
}

impl ProcedureState for ConfigureState {
    type Id = ConfigureStateId;

    fn id(&self) -> Self::Id {
        self.into()
    }
}
