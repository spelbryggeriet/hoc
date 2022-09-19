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

impl Cidr {
    const fn bits(&self) -> (u128, u32) {
        match self.ip_addr {
            IpAddr::V4(ipv4_addr) => (u32::from_be_bytes(ipv4_addr.octets()) as u128, 32),
            IpAddr::V6(ipv6_addr) => (u128::from_be_bytes(ipv6_addr.octets()), 128),
        }
    }

    pub fn start_address(&self) -> IpAddr {
        let (mut bits, num_bits) = self.bits();

        bits >>= num_bits - self.prefix_len;
        bits <<= num_bits - self.prefix_len;

        match self.ip_addr {
            IpAddr::V4(_) => IpAddr::from((bits as u32).to_be_bytes()),
            IpAddr::V6(_) => IpAddr::from(bits.to_be_bytes()),
        }
    }

    pub fn end_address(&self) -> IpAddr {
        let (mut bits, num_bits) = self.bits();

        bits >>= num_bits - self.prefix_len;
        bits += 1;
        bits <<= num_bits - self.prefix_len;

        match self.ip_addr {
            IpAddr::V4(_) => IpAddr::from((bits as u32).to_be_bytes()),
            IpAddr::V6(_) => IpAddr::from(bits.to_be_bytes()),
        }
    }

    pub fn contains(&self, ip_addr: IpAddr) -> bool {
        match (self.ip_addr, ip_addr) {
            (IpAddr::V4(_), IpAddr::V6(_)) | (IpAddr::V6(_), IpAddr::V4(_)) => {
                panic!(
                    "version of IP address `{ip_addr}` should match version of CIDR block `{self}`"
                )
            }
            _ => (),
        }

        ip_addr >= self.start_address() && ip_addr < self.end_address()
    }

    pub fn step(&self, step: u128) -> Option<IpAddr> {
        let (mut bits, _) = self.bits();

        bits += step;

        let address = match self.ip_addr {
            IpAddr::V4(_) => IpAddr::from((bits as u32).to_be_bytes()),
            IpAddr::V6(_) => IpAddr::from(bits.to_be_bytes()),
        };

        if address < self.end_address() {
            Some(address)
        } else {
            None
        }
    }
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
