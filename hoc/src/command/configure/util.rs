use std::{
    borrow::Cow,
    fmt::{self, Display, Formatter},
    net::Ipv4Addr,
};

use hoclog::LogErr;
use lazy_regex::regex_captures;
use osshkeys::{keys::FingerprintHash, Key, KeyPair, PublicParts};
use serde::{Deserialize, Serialize};

pub fn fingerprint_randomart(alg: FingerprintHash, k: &KeyPair) -> hoclog::Result<String> {
    const FLDBASE: usize = 8;
    const FLDSIZE_Y: usize = FLDBASE + 1;
    const FLDSIZE_X: usize = FLDBASE * 2 + 1;

    // Chars to be used after each other every time the worm intersects with itself.  Matter of
    // taste.
    const AUGMENTATION_CHARS: &[u8] = b" .o+=*BOX@%&#/^SE";

    let len = AUGMENTATION_CHARS.len() - 1;

    let mut art = String::with_capacity((FLDSIZE_X + 3) * (FLDSIZE_Y + 2));

    // Initialize field.
    let mut field = [[0; FLDSIZE_X]; FLDSIZE_Y];
    let mut x = FLDSIZE_X / 2;
    let mut y = FLDSIZE_Y / 2;

    // Process raw key.
    let dgst_raw = k.fingerprint(alg).log_err()?;
    for i in 0..dgst_raw.len() {
        // Each byte conveys four 2-bit move commands.
        let mut input = dgst_raw[i];
        for _ in 0..4 {
            // Evaluate 2 bit, rest is shifted later.
            x = if (input & 0x1) != 0 {
                x + 1
            } else {
                x.saturating_sub(1)
            };
            y = if (input & 0x2) != 0 {
                y + 1
            } else {
                y.saturating_sub(1)
            };

            // Assure we are still in bounds.
            x = x.min(FLDSIZE_X - 1);
            y = y.min(FLDSIZE_Y - 1);

            // Augment the field.
            if field[y][x] < len as u8 - 2 {
                field[y][x] += 1;
            }
            input >>= 2;
        }
    }

    // Mark starting point and end point.
    field[FLDSIZE_Y / 2][FLDSIZE_X / 2] = len as u8 - 1;
    field[y][x] = len as u8;

    // Assemble title.
    let title = format!("[{:?} {}]", k.keytype(), k.size());
    // If [type size] won't fit, then try [type]; fits "[ED25519-CERT]".
    let title = if title.chars().count() > FLDSIZE_X {
        format!("[{:?}]", k.keytype())
    } else {
        title
    };

    // Assemble hash ID.
    let hash = format!("[{:?}]", alg);

    // Output upper border.
    art += &format!("+{:-^width$}+\n", title, width = FLDSIZE_X);

    // Output content.
    for y in 0..FLDSIZE_Y {
        art.push('|');
        art.extend(
            field[y]
                .iter()
                .map(|&c| AUGMENTATION_CHARS[c as usize] as char),
        );
        art += "|\n";
    }

    // Output lower border.
    art += &format!("+{:-^width$}+", hash, width = FLDSIZE_X);

    Ok(art)
}

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
