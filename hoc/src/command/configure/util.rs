use std::{
    borrow::Cow,
    fmt::{self, Display, Formatter},
    net::Ipv4Addr,
};

use lazy_regex::regex_captures;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct LocalEndpoint {
    pub hostname: String,
    pub ip_address: Ipv4Addr,
    pub interface: String,
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

impl LocalEndpoint {
    pub fn parse_arp_output(output: &str, node_name: &str) -> (usize, Vec<Self>) {
        let mut default_index = None;

        let endpoints = output
            .lines()
            .enumerate()
            .map(move |(i, l)| {
                let (_, hostname, ip_address, interface) =
                    regex_captures!(r"^([^ ]*) \(([^)]*)\)[^\[]*\[([^]]*)\]$", l).unwrap();

                if default_index.is_none() && hostname.contains(node_name) {
                    default_index.replace(i);
                }

                LocalEndpoint {
                    hostname: hostname.into(),
                    ip_address: ip_address.parse().unwrap(),
                    interface: interface.into(),
                }
            })
            .collect();

        (default_index.unwrap_or_default(), endpoints)
    }

    pub fn host(&self) -> Cow<str> {
        if self.hostname != "?" {
            Cow::Borrowed(&self.hostname)
        } else {
            Cow::Owned(self.ip_address.to_string())
        }
    }
}
