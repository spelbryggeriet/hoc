use super::{SourceDiskInfo, SourceDiskPartitionInfo};

use nom::IResult;

pub fn fdisk_output(s: &str) -> IResult<&str, SourceDiskInfo> {
    use nom::bytes::complete::{tag, take_until};
    use nom::character::complete::{digit1, multispace1};
    use nom::combinator::map_res;
    use nom::multi::many1;
    use nom::sequence::preceded;

    let (s, _) = take_until("bytes")(s)?;
    let (s, num_sectors) = preceded(tag("bytes, "), map_res(digit1, str::parse))(s)?;
    let (s, _) = take_until("Sector size")(s)?;
    let (s, sector_size) = preceded(
        tag("Sector size (logical/physical): "),
        map_res(digit1, str::parse),
    )(s)?;
    let (s, _) = take_until("Type")(s)?;
    let (s, _) = tag("Type")(s)?;
    let (s, _) = multispace1(s)?;
    let (s, mut partitions) = many1(fdisk_partition_info)(s)?;

    partitions
        .iter_mut()
        .for_each(|p| p.sector_size = sector_size);

    Ok((
        s,
        SourceDiskInfo {
            num_sectors,
            partitions,
        },
    ))
}

fn fdisk_partition_info(s: &str) -> IResult<&str, SourceDiskPartitionInfo> {
    use nom::bytes::complete::take_until;
    use nom::character::complete::{digit1, multispace1, space1};
    use nom::combinator::{map, map_res};

    let (s, start_sector) = map_res(digit1, str::parse)(s)?;
    let (s, _) = space1(s)?;
    let (s, num_sectors) = map_res(digit1, str::parse)(s)?;
    let (s, _) = space1(s)?;
    let (s, name) = map(take_until("\n"), |s: &str| s.trim().to_string())(s)?;
    let (s, _) = multispace1(s)?;

    Ok((
        s,
        SourceDiskPartitionInfo {
            name,
            num_sectors,
            start_sector,
            ..Default::default()
        },
    ))
}
