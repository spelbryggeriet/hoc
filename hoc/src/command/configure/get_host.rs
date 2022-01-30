use std::fmt::{self, Display, Formatter};

use lazy_regex::regex_captures;

use super::*;

impl Configure {
    pub(super) fn get_host(&self, step: &mut ProcedureStep) -> Result<Halt<ConfigureState>> {
        let local_endpoint = status!("Finding local endpoints", {
            let output = cmd!("arp", "-a").hide_output().run()?;
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

        let host = if local_endpoint.hostname != "?" {
            local_endpoint.hostname
        } else {
            local_endpoint.ip_address.to_string()
        };

        Ok(Halt::persistent_yield(ConfigureState::ChangeDefaultUser {
            host,
        }))
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct LocalEndpoint {
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
