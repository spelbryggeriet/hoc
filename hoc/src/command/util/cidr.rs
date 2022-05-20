use std::{
    fmt::{self, Display, Formatter},
    net::{AddrParseError, IpAddr},
    num::ParseIntError,
    str::FromStr,
};

use serde::{de::Visitor, Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("expected version {_0}, got {_1}")]
    WrongVersion(i32, i32),
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

#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct Cidr {
    pub ip_addr: IpAddr,
    pub prefix_len: u32,
}

impl Cidr {
    pub fn contains(&self, ip_addr: &IpAddr) -> Result<bool, Error> {
        match (self.ip_addr, ip_addr) {
            (IpAddr::V4(_), IpAddr::V6(_)) => return Err(Error::WrongVersion(4, 6)),
            (IpAddr::V6(_), IpAddr::V4(_)) => return Err(Error::WrongVersion(6, 4)),
            _ => (),
        }

        let (num_bits, mut start_bits) = match self.ip_addr {
            IpAddr::V4(ipv4_addr) => (32, u32::from_be_bytes(ipv4_addr.octets()) as u128),
            IpAddr::V6(ipv6_addr) => (128, u128::from_be_bytes(ipv6_addr.octets())),
        };

        start_bits >>= num_bits - self.prefix_len;
        let mut end_bits = start_bits + 1;
        start_bits <<= num_bits - self.prefix_len;
        end_bits <<= num_bits - self.prefix_len;

        let (start_address, end_address) = match self.ip_addr {
            IpAddr::V4(_) => (
                IpAddr::from((start_bits as u32).to_be_bytes()),
                IpAddr::from((end_bits as u32).to_be_bytes()),
            ),
            IpAddr::V6(_) => (
                IpAddr::from(start_bits.to_be_bytes()),
                IpAddr::from(end_bits.to_be_bytes()),
            ),
        };

        Ok(ip_addr >= &start_address && ip_addr < &end_address)
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
        let ip_addr: IpAddr = ip_addr_str.parse()?;
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

impl Serialize for Cidr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Cidr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct CidrVisitor;

        impl<'de> Visitor<'de> for CidrVisitor {
            type Value = Cidr;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("CIDR block")
            }

            fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                s.parse().map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_str(CidrVisitor)
    }
}
