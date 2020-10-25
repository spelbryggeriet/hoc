use nom::IResult;

pub fn last_ansi_escape_code(s: &str) -> Option<String> {
    use nom::multi::many0;

    many0(next_ansi_escape_code)(s)
        .ok()
        .and_then(|(_, mut v)| v.pop().flatten())
}

fn next_ansi_escape_code(s: &str) -> IResult<&str, Option<String>> {
    use nom::{branch::alt, bytes::complete::take_until, combinator::opt};

    let (s, _) = take_until("\u{1b}")(s)?;
    opt(alt((ansi_csi_single, ansi_csi_multi)))(s)
}

fn ansi_csi_single(s: &str) -> IResult<&str, String> {
    use nom::bytes::complete::{tag, take_while_m_n};
    use nom::character::complete::digit1;

    let (s, start) = tag("\u{1b}[")(s)?;
    let (s, digits) = digit1(s)?;
    let (s, end) = take_while_m_n(1, 1, |c: char| c.is_ascii_alphabetic())(s)?;

    Ok((s, [start, digits, end].join("")))
}

fn ansi_csi_multi(s: &str) -> IResult<&str, String> {
    use nom::bytes::complete::{tag, take_while_m_n};
    use nom::character::complete::digit1;

    let (s, start) = tag("\u{1b}[")(s)?;
    let (s, first_digits) = digit1(s)?;
    let (s, colon) = tag(";")(s)?;
    let (s, second_digits) = digit1(s)?;
    let (s, end) = take_while_m_n(1, 1, |c: char| c.is_ascii_alphabetic())(s)?;

    Ok((s, [start, first_digits, colon, second_digits, end].join("")))
}
