use std::{
    fmt::{self, Display, Formatter},
    net::{AddrParseError, IpAddr},
    num::ParseIntError,
    str::FromStr,
};

use thiserror::Error;

#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct Cidr {
    pub ip_addr: IpAddr,
    pub prefix_len: u32,
}

impl Display for Cidr {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let ip_addr = self.ip_addr;
        let prefix_len = self.prefix_len;
        write!(f, "{ip_addr}/{prefix_len}")
    }
}

impl FromStr for Cidr {
    type Err = CidrParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (ip_addr_str, prefix_len_str) = s
            .split_once("/")
            .ok_or_else(|| CidrParseError::MissingSlash)?;
        let ip_addr: std::net::IpAddr = ip_addr_str.parse()?;
        let prefix_len: u32 = prefix_len_str.parse()?;

        let prefix_len_bound = if ip_addr.is_ipv4() { 32 } else { 128 };

        if prefix_len <= prefix_len_bound {
            Ok(Cidr {
                ip_addr,
                prefix_len,
            })
        } else {
            Err(CidrParseError::PrefixLenOutOfRange {
                prefix_len,
                prefix_len_bound,
            })
        }
    }
}

#[derive(Error, Debug)]
pub enum CidrParseError {
    #[error("expected '/' separator")]
    MissingSlash,

    #[error(transparent)]
    IpAddr(#[from] AddrParseError),

    #[error("prefix length is not a valid integer: {0}")]
    InvalidPrefixLen(#[from] ParseIntError),

    #[error("prefix length needs to be between 0 and {prefix_len_bound}, got {prefix_len}")]
    PrefixLenOutOfRange {
        prefix_len: u32,
        prefix_len_bound: u32,
    },
}
