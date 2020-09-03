use super::{DiskInfo, DiskPartitionInfo, DriveInfo, DrivePartitionInfo, Size};
use nom::IResult;

pub(super) fn drive_info(s: &str) -> IResult<&str, Vec<DriveInfo>> {
    nom::multi::many0(single_drive_info)(s)
}

fn single_drive_info(s: &str) -> IResult<&str, DriveInfo> {
    use nom::bytes::complete::{tag, take_till, take_while};
    use nom::{combinator::map, multi::many1};

    let (s, dir) = map(tag("/dev/"), str::to_string)(s)?;
    let (s, id) = map(take_till(char::is_whitespace), str::to_string)(s)?;
    let (s, _) =
        take_while(|c: char| c.is_alphabetic() || c.is_whitespace() || c.is_ascii_punctuation())(
            s,
        )?;
    let (s, partitions) = many1(drive_partition_info)(s)?;

    Ok((
        s,
        DriveInfo {
            dir,
            id,
            partitions,
        },
    ))
}

fn drive_partition_info(s: &str) -> IResult<&str, DrivePartitionInfo> {
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
        DrivePartitionInfo {
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

pub(super) fn disk_info(s: &str) -> IResult<&str, DiskInfo> {
    use nom::bytes::complete::{tag, take_till, take_until};
    use nom::character::complete::digit1;
    use nom::multi::separated_list;
    use nom::{combinator::map_res, multi::many1};

    let (s, _) = take_until("geometry: ")(s)?;
    let (s, _) = tag("geometry: ")(s)?;
    let (s, _) = separated_list(tag("/"), digit1)(s)?;
    let (s, _) = tag(" [")(s)?;
    let (s, num_sectors) = map_res(digit1, str::parse::<u64>)(s)?;
    let (s, _) = take_until("size]")(s)?;
    let (s, _) = take_till(|c: char| c.is_digit(10))(s)?;
    let (s, partitions) = many1(disk_partition_info)(s)?;

    Ok((
        s,
        DiskInfo {
            num_sectors,
            partitions,
        },
    ))
}

// Disk: <...>  geometry: 785/128/63 [6332416 sectors]
// Signature: 0xAA55
//          Starting       Ending
//  #: id  cyl  hd sec -  cyl  hd sec [     start -       size]
// ------------------------------------------------------------------------
//  1: 0C   16  16   1 -  321  64   2 [      8192 -     155648] Win95 FAT32L
// *2: 83  321  65   1 - 1023 254   2 [    163840 -     999424] Linux files*
//  3: 83 1023 254   2 - 1023 254   2 [   1163264 -    4884480] Linux files*
//  4: 00    0   0   0 -    0   0   0 [         0 -          0] unused
fn disk_partition_info(s: &str) -> IResult<&str, DiskPartitionInfo> {
    use nom::bytes::complete::{tag, take_till, take_until};
    use nom::character::complete::digit1;
    use nom::combinator::{map, map_res};

    let (s, index) = map_res(digit1, str::parse::<u64>)(s)?;
    let (s, _) = take_until("[")(s)?;
    let (s, _) = take_till(|c: char| c.is_digit(10))(s)?;
    let (s, start_sector) = map_res(digit1, str::parse::<u64>)(s)?;
    let (s, _) = take_till(|c: char| c.is_digit(10))(s)?;
    let (s, num_sectors) = map_res(digit1, str::parse::<u64>)(s)?;
    let (s, _) = tag("] ")(s)?;
    let (s, name) = map(take_until("\n"), |s: &str| s.trim().to_string())(s)?;
    let (s, _) = take_till(|c: char| c.is_digit(10))(s)?;

    Ok((s, DiskPartitionInfo {
        index,
        name,
        num_sectors,
        start_sector,
        ..Default::default()
    }))
}
