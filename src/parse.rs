use crate::{DiskInfo, PartitionInfo, Size};
use anyhow::anyhow;
use nom::IResult;

pub(crate) fn disk_info(s: &str) -> anyhow::Result<Vec<DiskInfo>> {
    use nom::multi::many0;

    let (_, disk_info) = many0(single_disk_info)(s).map_err(|e| anyhow!(e.to_string()))?;

    Ok(disk_info)
}

fn single_disk_info(s: &str) -> IResult<&str, DiskInfo> {
    use nom::bytes::complete::{tag, take_till, take_while};
    use nom::{combinator::map, multi::many1};

    let (s, dir) = map(tag("/dev/"), str::to_string)(s)?;
    let (s, id) = map(take_till(char::is_whitespace), str::to_string)(s)?;
    let (s, _) =
        take_while(|c: char| c.is_alphabetic() || c.is_whitespace() || c.is_ascii_punctuation())(
            s,
        )?;
    let (s, mut partitions) = many1(partition_info)(s)?;
    let last_partition = partitions.pop().unwrap();

    Ok((
        s,
        DiskInfo {
            dir,
            id,
            partitions,
            last_partition,
        },
    ))
}

fn partition_info(s: &str) -> IResult<&str, PartitionInfo> {
    use nom::bytes::complete::tag;
    use nom::character::complete::{digit1, multispace0, space1};
    use nom::combinator::{map, map_res};
    use nom::sequence::tuple;

    let (s, index) = map_res(digit1, str::parse::<u32>)(s)?;
    let (s, _) = tuple((tag(":"), space1))(s)?;
    let (s, part_type) = valid_name(s)?;
    let (s, _) = tag(" ")(s)?;
    let (s, name) = map(valid_name, |name| Some(name).filter(|n| !n.is_empty()))(s)?;
    let (s, _) = space1(s)?;
    let (s, size) = size(s)?;
    let (s, _) = space1(s)?;
    let (s, id) = valid_name(s)?;
    let (s, _) = multispace0(s)?;

    Ok((
        s,
        PartitionInfo {
            index,
            part_type,
            name,
            size,
            id,
        },
    ))
}

fn size(s: &str) -> IResult<&str, Size> {
    use nom::bytes::complete::{tag, take_while};
    use nom::{branch::alt, combinator::map_res};

    let (s, marker) = take_while(|c: char| c.is_ascii_punctuation())(s)?;
    let (s, num) = map_res(
        take_while(|c: char| c.is_digit(10) || c == '.'),
        |num: &str| num.parse::<f32>(),
    )(s)?;
    let (s, _) = tag(" ")(s)?;
    let (s, unit) = alt((tag("KB"), tag("MB"), tag("GB"), tag("TB")))(s)?;

    Ok((s, (marker.to_string(), num, unit.to_string())))
}

fn valid_name(s: &str) -> IResult<&str, String> {
    use nom::{bytes::complete::take_till, combinator::map};

    map(take_till(char::is_whitespace), str::to_string)(s)
}
