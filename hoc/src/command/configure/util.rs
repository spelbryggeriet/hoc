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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fingerprint_randomart() {
        const ART: &str = "\
            +--[ED25519 256]--+\n\
            |      ..o o..oo=B|\n\
            |     . . B ooo=oE|\n\
            |      . * o =..o |\n\
            |       o = *     |\n\
            |    .   S * o    |\n\
            |     + o = =     |\n\
            |    . = . =      |\n\
            |     . o...o     |\n\
            |      .o==o.     |\n\
            +----[SHA256]-----+";
        const KEYPAIR: &str = "\
            -----BEGIN OPENSSH PRIVATE KEY-----\n\
            b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\n\
            QyNTUxOQAAACDhSeGEnFgvaAVu9TWWDbdI5qHN/fUm3YRDci19WcfYQgAAAKCp1NZoqdTW\n\
            aAAAAAtzc2gtZWQyNTUxOQAAACDhSeGEnFgvaAVu9TWWDbdI5qHN/fUm3YRDci19WcfYQg\n\
            AAAEDCKVoVjFBKe2trTOEL5PGUSpzk2DdTjwhr7k+FIzX90uFJ4YScWC9oBW71NZYNt0jm\n\
            oc399SbdhENyLX1Zx9hCAAAAGmxpZGluQEhhbXB1cy1NQlAuaGstcm91dGVyAQID\n\
            -----END OPENSSH PRIVATE KEY-----";

        let key_pair = KeyPair::from_keystr(KEYPAIR, None).unwrap();
        let generated_art = fingerprint_randomart(FingerprintHash::SHA256, &key_pair).unwrap();

        assert_eq!(ART, generated_art);
    }
}
