use std::net::Ipv4Addr;

use nom::IResult;

use super::LocalEndpoint;

pub fn arp_output(s: &str) -> IResult<&str, Vec<LocalEndpoint>> {
    nom::multi::many0(local_endpoint)(s)
}

pub fn local_endpoint(s: &str) -> IResult<&str, LocalEndpoint> {
    if cfg!(target_os = "macos") {
        use nom::bytes::complete::{tag, take_until, take_while};
        use nom::character::complete::multispace1;
        use nom::combinator::{map, map_res};

        let (s, hostname) = map(take_until(" "), |s: &str| {
            Some(s.to_string()).filter(|v| v != "?")
        })(s)?;
        let (s, _) = tag(" (")(s)?;
        let (s, ip_address) = map_res(
            take_while(|c: char| c.is_ascii_digit() || c == '.'),
            str::parse::<Ipv4Addr>,
        )(s)?;
        let (s, _) = take_until("[")(s)?;
        let (s, _) = tag("[")(s)?;
        let (s, interface) = map(take_until("]"), str::to_string)(s)?;
        let (s, _) = tag("]")(s)?;
        let (s, _) = multispace1(s)?;

        Ok((
            s,
            LocalEndpoint {
                hostname,
                ip_address,
                interface,
            },
        ))
    } else if cfg!(target_os = "linux") {
        unimplemented!();
    } else {
        unimplemented!();
    }
}
