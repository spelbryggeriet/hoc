use std::collections::VecDeque;

use crate::HocValue;

const BSLASH: char = '\\';
const COLON: char = ':';
const COMMA: char = ',';
const DQUOTE: char = '"';
const ESIGN: char = '=';
const LBRACKET: char = '[';
const LPAREN: char = '(';
const RBRACKET: char = ']';
const RPAREN: char = ')';

const EMPTY_LIST: &str = "[]";

struct Utf8IndicesIter<I> {
    inner: I,
    index: usize,
}

impl<I> Iterator for Utf8IndicesIter<I>
where
    I: Iterator<Item = char>,
{
    type Item = (usize, char);

    fn next(&mut self) -> Option<Self::Item> {
        let c = self.inner.next()?;
        let cur = self.index;
        self.index += c.len_utf8();
        Some((cur, c))
    }
}

trait Utf8IndicesIterExt: Iterator<Item = char>
where
    Self: Sized,
{
    fn utf8_indices(self) -> Utf8IndicesIter<Self> {
        Utf8IndicesIter {
            inner: self,
            index: 0,
        }
    }
}

impl<I> Utf8IndicesIterExt for I where I: Iterator<Item = char> {}

#[derive(Error, Debug)]
#[error("Failed parsing Hoc line: {message}")]
pub struct HocLineParseError {
    message: String,
}

impl HocLineParseError {
    pub fn new(message: impl ToString) -> Self {
        Self {
            message: message.to_string(),
        }
    }
}

pub fn parse_hoc_line(
    line: &str,
) -> Result<
    Option<(
        &'static str,
        &'static str,
        VecDeque<(&str, Option<HocValue>)>,
    )>,
    HocLineParseError,
> {
    let s = if let Some(s) = consume_hoc_prefix(line) {
        s
    } else {
        return Ok(None);
    };

    let (s, ns) = parse_hoc_namespace(s)?;
    let (s, cmd) = parse_hoc_command(s, ns)?;
    let (s, args) = parse_hoc_command_arguments(s, ns, cmd)?;

    if s.is_empty() {
        Ok(Some((ns, cmd, args)))
    } else {
        Err(HocLineParseError::new(format!(
            "trailing characters '{}'",
            s,
        )))
    }
}

fn consume_hoc_prefix(s: &str) -> Option<&str> {
    const PREFIX: &str = "[hoc]:";

    if !s.starts_with(PREFIX) {
        return None;
    }

    Some(&s[PREFIX.len()..])
}

fn parse_hoc_namespace(s: &str) -> Result<(&str, &'static str), HocLineParseError> {
    for ns in super::COMMANDS.iter().map(|(ns, _)| ns) {
        if s.starts_with(ns) && s[ns.len()..].starts_with(COLON) {
            return Ok((&s[ns.len() + COLON.len_utf8()..], ns));
        }
    }

    let end = s
        .chars()
        .utf8_indices()
        .find_map(|(i, c)| (c == COLON).then(|| i))
        .unwrap_or(s.len());

    Err(HocLineParseError::new(format!(
        "unknown namespace '{}'",
        &s[..end]
    )))
}

fn parse_hoc_command<'s>(
    s: &'s str,
    ns: &str,
) -> Result<(&'s str, &'static str), HocLineParseError> {
    for cmd in super::COMMANDS
        .iter()
        .filter_map(|(some_ns, cmds)| (ns == *some_ns).then(|| cmds.iter()))
        .flatten()
        .map(|(cmd, _)| cmd)
    {
        if s.starts_with(cmd) && s[cmd.len()..].starts_with(LPAREN) {
            return Ok((&s[cmd.len()..], cmd));
        }
    }

    let end = s
        .chars()
        .utf8_indices()
        .find_map(|(i, c)| (c == LPAREN).then(|| i))
        .unwrap_or(s.len());

    return Err(HocLineParseError::new(format!(
        "unknown command '{}' in namespace '{}'",
        &s[..end],
        ns
    )));
}

fn parse_hoc_command_arguments<'a>(
    mut s: &'a str,
    ns: &str,
    cmd: &str,
) -> Result<(&'a str, VecDeque<(&'a str, Option<HocValue>)>), HocLineParseError> {
    const PREFIX: &str = "failed parsing command arguments";

    if !s.starts_with(LPAREN) {
        return Err(HocLineParseError::new(format!(
            "{}: expected left parenthesis character '{}'",
            PREFIX, LPAREN
        )));
    }

    s = &s[LPAREN.len_utf8()..];

    let mut parsed_args = VecDeque::new();
    for expected_arg in super::COMMANDS
        .iter()
        .filter_map(|(some_ns, cmds)| (*some_ns == ns).then(|| cmds.iter()))
        .flatten()
        .filter_map(|(some_cmd, args)| (*some_cmd == cmd).then(|| args.iter()))
        .flatten()
    {
        let end = s
            .chars()
            .utf8_indices()
            .find_map(|(i, c)| {
                (expected_arg.is_none() && ([COMMA, RPAREN].contains(&c))
                    || expected_arg.is_some() && c == ESIGN)
                    .then(|| i)
            })
            .ok_or_else(|| {
                if expected_arg.is_none() {
                    HocLineParseError::new(format!(
                        "{}: expected on of the characters: comma '{}' or right parenthesis '{}'",
                        PREFIX, COMMA, RPAREN,
                    ))
                } else {
                    HocLineParseError::new(format!(
                        "{}: expected equal sign character '{}'",
                        PREFIX, ESIGN
                    ))
                }
            })?;

        let key = &s[..end];
        s = &s[end..];

        match expected_arg {
            Some(arg_type) => {
                s = &s[ESIGN.len_utf8()..];
                if *arg_type == super::VALUE {
                    let (s_new, value) = parse_hoc_value(s)?;
                    parsed_args.push_back((key, Some(value)));
                    s = s_new;
                } else if *arg_type == super::STRING {
                    let (s_new, string) = parse_hoc_string(s)?;
                    parsed_args.push_back((key, Some(HocValue::String(string))));
                    s = s_new;
                } else if *arg_type == super::LIST {
                    let (s_new, list) = parse_hoc_list(s)?;
                    parsed_args.push_back((key, Some(HocValue::List(list))));
                    s = s_new;
                } else {
                    unreachable!();
                }

                if !s.starts_with(|c| [COMMA, RPAREN].contains(&c)) {
                    return Err(HocLineParseError::new(format!(
                        "{}: expected on of the characters: comma '{}' or right parenthesis '{}'",
                        PREFIX, COMMA, RPAREN,
                    )));
                }
            }
            None => {
                parsed_args.push_back((key, None));
            }
        }

        if s.starts_with(COMMA) {
            s = &s[COMMA.len_utf8()..];
        }
    }

    if !s.starts_with(RPAREN) {
        return Err(HocLineParseError::new(format!(
            "{}: expected right parenthesis character '{}'",
            PREFIX, RPAREN
        )));
    }

    s = &s[RPAREN.len_utf8()..];

    Ok((s, parsed_args))
}

fn parse_hoc_value(s: &str) -> Result<(&str, HocValue), HocLineParseError> {
    const PREFIX: &str = "failed parsing value";

    if s.starts_with(DQUOTE) {
        parse_hoc_string(s).map(|(s_new, string)| (s_new, HocValue::String(string)))
    } else if s.starts_with(LBRACKET) {
        parse_hoc_list(s).map(|(s_new, list)| (s_new, HocValue::List(list)))
    } else if s.len() == 0 {
        Err(HocLineParseError::new(format!("{}: empty value", PREFIX)))
    } else {
        Err(HocLineParseError::new(format!(
            "{}: unexpected character '{}'",
            PREFIX,
            s.chars().next().unwrap()
        )))
    }
}

fn parse_hoc_string(s: &str) -> Result<(&str, String), HocLineParseError> {
    const PREFIX: &str = "failed parsing string";

    if !s.starts_with(DQUOTE) {
        return Err(HocLineParseError::new(format!(
            "{}: expected double quote character '{}'",
            PREFIX, DQUOTE
        )));
    }

    let mut chars = s.chars().utf8_indices().skip(1);

    let mut value = String::new();

    while let Some((i, c)) = chars.next() {
        if c == BSLASH {
            if let Some((_, next_c)) = chars.next() {
                if ![BSLASH, DQUOTE].contains(&next_c) {
                    return Err(HocLineParseError::new(format!(
                        "{}: unknown character escape '{}'",
                        PREFIX, next_c,
                    )));
                } else {
                    value.push(next_c);
                }
            } else {
                return Err(HocLineParseError::new(format!(
                    "{}: trailing backslash",
                    PREFIX,
                )));
            }
        } else if c == DQUOTE {
            return Ok((&s[i + DQUOTE.len_utf8()..], value));
        } else {
            value.push(c);
        }
    }

    Err(HocLineParseError::new(format!(
        "{}: unclosed double quote",
        PREFIX
    )))
}

fn parse_hoc_list(mut s: &str) -> Result<(&str, Vec<HocValue>), HocLineParseError> {
    const PREFIX: &str = "failed parsing list";

    if s.starts_with(EMPTY_LIST) {
        return Ok((&s[EMPTY_LIST.len()..], Vec::new()));
    }

    if !s.starts_with(LBRACKET) {
        return Err(HocLineParseError::new(format!(
            "{}: expected left bracket character '{}'",
            PREFIX, LBRACKET
        )));
    }

    let mut v = Vec::new();

    s = &s[LBRACKET.len_utf8()..];
    loop {
        let (s_new, value) = parse_hoc_value(s)?;
        v.push(value);

        if s_new.starts_with(RBRACKET) {
            return Ok((&s_new[RBRACKET.len_utf8()..], v));
        } else if s_new.len() == 0 {
            return Err(HocLineParseError::new(format!(
                "{}: unclosed bracket",
                PREFIX
            )));
        } else if !s_new.starts_with(COMMA) {
            return Err(HocLineParseError::new(format!(
                "{}: trailing characters '{}'",
                PREFIX, s_new,
            )));
        }

        s = &s_new[COMMA.len_utf8()..];
    }
}
