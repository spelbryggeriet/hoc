use std::{
    fmt::{self, Display, Formatter},
    net::Ipv4Addr,
};

use lazy_regex::regex_captures;

use super::*;

#[derive(Debug, Serialize, Deserialize)]
pub struct LocalEndpoint {
    hostname: String,
    ip_address: Ipv4Addr,
    interface: String,
}

impl Display for LocalEndpoint {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "{} / {} / {}",
            self.hostname, self.ip_address, self.interface
        )
    }
}

impl Configure {
    pub(super) fn get_local_endpoints(
        &self,
        step: &mut ProcedureStep,
    ) -> Result<Halt<ConfigureState>> {
        let local_endpoint = status!("Finding local endpoints", {
            let output = cmd_capture!("arp", "-a")?;
            let mut default_index = None;

            let mut endpoints: Vec<_> = output
                .lines()
                .enumerate()
                .map(move |(i, l)| {
                    let (_, hostname, ip_address, interface) =
                        regex_captures!(r"^([^ ]*) \(([^)]*)\)[^\[]*\[([^]]*)\]$", l).unwrap();

                    if default_index.is_none() && hostname.contains(&self.node_name) {
                        default_index = Some(i);
                    }

                    LocalEndpoint {
                        hostname: hostname.into(),
                        ip_address: ip_address.parse().unwrap(),
                        interface: interface.into(),
                    }
                })
                .collect();

            let index = choose!(
                "Which endpoint do you want to configure?",
                items = &endpoints,
                default_index = default_index.unwrap_or_default(),
            )?;

            endpoints.remove(index)
        });

        Ok(Halt::persistent_yield(ConfigureState::NodeSettings {
            local_endpoint,
        }))
    }
}
