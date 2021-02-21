#[macro_use]
extern crate thiserror;

mod hocfile;
mod tree;

use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    unreachable,
};

pub use hocfile::*;

pub type HocState = HashMap<String, HocValue>;

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

const VALUE: &str = "value";
const STRING: &str = "string";
const LIST: &str = "list";

const COMMANDS: &[(&str, &[(&str, &[Option<&str>])])] = &[
    (
        "out",
        &[("set", &[Some(VALUE)]), ("append", &[Some(VALUE)])],
    ),
    (
        "in",
        &[("unset", &[None]), ("choose", &[Some(STRING), Some(LIST)])],
    ),
];

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

trait PopHocCommandArgument<'a> {
    fn get_front(&mut self) -> Option<(&'a str, Option<HocValue>)>;
    fn get_front_ref(&mut self) -> Option<(&'a str, Option<&HocValue>)>;

    fn peek_checked(&mut self) -> Result<(&'a str, Option<&HocValue>), String> {
        self.get_front_ref()
            .ok_or_else(|| "expected more arguments".into())
    }

    fn pop_key_checked(&mut self) -> Result<&'a str, String> {
        let (key, value) = self
            .get_front()
            .ok_or_else(|| "expected more arguments".to_string())?;
        if !value.is_some() {
            Ok(key)
        } else {
            Err("expected key only, found key-value pair".into())
        }
    }

    fn pop_key_value_checked(&mut self) -> Result<(&'a str, HocValue), String> {
        let (key, value) = self.get_front().ok_or("expected more arguments")?;
        Ok((key, value.ok_or_else(|| "expected value")?))
    }

    fn pop_key_string_checked(&mut self) -> Result<(&'a str, String), String> {
        let (key, value) = self.get_front().ok_or("expected more arguments")?;
        Ok((
            key,
            value
                .and_then(|v| v.as_string().ok())
                .ok_or_else(|| "expected string")?,
        ))
    }

    fn pop_key_list_checked(&mut self) -> Result<(&'a str, Vec<HocValue>), String> {
        let (key, value) = self.get_front().ok_or("expected more arguments")?;
        Ok((
            key,
            value
                .and_then(|v| v.as_list().ok())
                .ok_or_else(|| "expected list")?,
        ))
    }

    fn pop_value_for_key_checked(&mut self, key: &str) -> Result<HocValue, String> {
        let (some_key, value) = self.pop_key_value_checked()?;
        if some_key == key {
            Ok(value)
        } else {
            Err(format!("expected key '{}'", key))
        }
    }

    fn pop_string_for_key_checked(&mut self, key: &str) -> Result<String, String> {
        let (some_key, string) = self.pop_key_string_checked()?;
        if some_key == key {
            Ok(string)
        } else {
            Err(format!("expected key '{}'", key))
        }
    }

    fn pop_list_for_key_checked(&mut self, key: &str) -> Result<Vec<HocValue>, String> {
        let (some_key, list) = self.pop_key_list_checked()?;
        if some_key == key {
            Ok(list)
        } else {
            Err(format!("expected key '{}'", key))
        }
    }

    fn peek(&mut self) -> (&'a str, Option<&HocValue>) {
        self.peek_checked().unwrap_or_else(|e| panic!("{}", e))
    }

    fn pop_key(&mut self) -> &'a str {
        self.pop_key_checked().unwrap_or_else(|e| panic!("{}", e))
    }

    fn pop_key_value(&mut self) -> (&'a str, HocValue) {
        self.pop_key_value_checked()
            .unwrap_or_else(|e| panic!("{}", e))
    }

    fn pop_key_string(&mut self) -> (&'a str, String) {
        self.pop_key_string_checked()
            .unwrap_or_else(|e| panic!("{}", e))
    }

    fn pop_key_list(&mut self) -> (&'a str, Vec<HocValue>) {
        self.pop_key_list_checked()
            .unwrap_or_else(|e| panic!("{}", e))
    }

    fn pop_value_for_key(&mut self, key: &str) -> HocValue {
        self.pop_value_for_key_checked(key)
            .unwrap_or_else(|e| panic!("{}", e))
    }

    fn pop_string_for_key(&mut self, key: &str) -> String {
        self.pop_string_for_key_checked(key)
            .unwrap_or_else(|e| panic!("{}", e))
    }

    fn pop_list_for_key(&mut self, key: &str) -> Vec<HocValue> {
        self.pop_list_for_key_checked(key)
            .unwrap_or_else(|e| panic!("{}", e))
    }
}

impl<'a> PopHocCommandArgument<'a> for VecDeque<(&'a str, Option<HocValue>)> {
    fn get_front(&mut self) -> Option<(&'a str, Option<HocValue>)> {
        self.pop_front()
    }

    fn get_front_ref(&mut self) -> Option<(&'a str, Option<&HocValue>)> {
        self.front().map(|(k, v)| (*k, v.as_ref()))
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(untagged)]
pub enum HocValue {
    String(String),
    List(Vec<HocValue>),
}

impl HocValue {
    pub fn as_string(self) -> Result<String, Self> {
        match self {
            Self::String(s) => Ok(s),
            _ => Err(self),
        }
    }

    pub fn as_string_ref(&self) -> Option<&String> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_string_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_list(self) -> Result<Vec<Self>, Self> {
        match self {
            Self::List(l) => Ok(l),
            _ => Err(self),
        }
    }

    pub fn as_list_ref(&self) -> Option<&Vec<Self>> {
        match self {
            Self::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_list_mut(&mut self) -> Option<&mut Vec<Self>> {
        match self {
            Self::List(l) => Some(l),
            _ => None,
        }
    }
}

#[derive(Error, Debug)]
#[error("failed parsing Hoc line: {message}")]
pub struct HocLineParseError {
    message: String,
}

impl HocLineParseError {
    fn new(message: impl ToString) -> Self {
        Self {
            message: message.to_string(),
        }
    }
}

pub fn parse_hoc_line(
    input: &mut HocState,
    output: &mut HocState,
    line: &str,
) -> Result<bool, HocLineParseError> {
    let s = if let Some(s) = consume_hoc_prefix(line) {
        s
    } else {
        return Ok(false);
    };

    let (s, ns) = parse_hoc_namespace(s)?;
    let (s, cmd) = parse_hoc_command(s, ns)?;
    let (s, mut args) = parse_hoc_command_arguments(s, ns, cmd)?;

    let prefix = format!("executing command '{}' in namespace '{}'", cmd, ns);

    let modified =
        match (ns, cmd) {
            ("out", "set") => {
                let (key, value) = args.pop_key_value();
                output.insert(key.to_string(), value);
                true
            }

            ("out", "append") => {
                let key = args.peek().0;

                let existing = output.get_mut(key).ok_or_else(|| {
                    HocLineParseError::new(format!("{}: uninitialized field '{}'", prefix, key))
                })?;

                match existing {
                    HocValue::List(existing) => {
                        existing.push(args.pop_value_for_key(key));
                    }
                    HocValue::String(existing) => {
                        existing.push_str(&args.pop_string_for_key_checked(key).map_err(
                            |err| HocLineParseError::new(format!("{}: {}", prefix, err)),
                        )?);
                    }
                }

                true
            }

            ("in", "unset") => {
                let key = args.pop_key();

                input.remove(key).ok_or_else(|| {
                    HocLineParseError::new(format!("{}: '{}' is not defined", prefix, key))
                })?;

                true
            }

            ("in", "choose") => {
                println!("{:#?}", args);
                let message = args
                    .pop_string_for_key_checked("message")
                    .map_err(|err| HocLineParseError::new(format!("{}: {}", prefix, err)))?;
                let options = args
                    .pop_list_for_key_checked("options")
                    .map_err(|err| HocLineParseError::new(format!("{}: {}", prefix, err)))?;

                // hoclog::choose!(
                //     message,
                //     options.iter(),
                //     default_index,
                // );

                false
            }

            (ns, cmd) => unreachable!("did not expect command '{}' in namespace '{}'", cmd, ns),
        };

    if s.is_empty() {
        Ok(modified)
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

fn parse_hoc_namespace(s: &str) -> Result<(&str, &str), HocLineParseError> {
    for ns in COMMANDS.iter().map(|(ns, _)| ns) {
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

fn parse_hoc_command<'s, 'ns>(
    s: &'s str,
    ns: &'ns str,
) -> Result<(&'s str, &'ns str), HocLineParseError> {
    for cmd in COMMANDS
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
    for expected_arg in COMMANDS
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
                println!("{}", s);
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
                if *arg_type == VALUE {
                    let (s_new, value) = parse_hoc_value(s)?;
                    parsed_args.push_back((key, Some(value)));
                    s = s_new;
                } else if *arg_type == STRING {
                    let (s_new, string) = parse_hoc_string(s)?;
                    parsed_args.push_back((key, Some(HocValue::String(string))));
                    s = s_new;
                } else if *arg_type == LIST {
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
