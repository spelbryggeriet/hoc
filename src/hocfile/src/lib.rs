#[macro_use]
extern crate thiserror;

mod hocfile;
mod tree;

use serde::Serialize;
use std::collections::HashMap;

pub use hocfile::*;

pub type HocState = HashMap<String, HocValue>;

#[derive(Serialize, Clone)]
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
    const PREFIX: &str = "[hoc]:";

    if !line.starts_with(PREFIX) {
        return Ok(false);
    }

    let split: Vec<_> = line[PREFIX.len()..].splitn(2, ":").collect();

    let (ns, split) = match split.as_slice() {
        [ns @ "in", rest] | [ns @ "out", rest] => (*ns, rest.splitn(2, ":").collect::<Vec<_>>()),
        [unknown, _] => {
            return Err(HocLineParseError::new(format!(
                "unknown namespace '{}'",
                unknown
            )));
        }
        [ns] => {
            return Err(HocLineParseError::new(format!(
                "namespace '{}' is missing a command",
                ns
            )));
        }
        _ => unreachable!(),
    };

    let (cmd, split) = match split.as_slice() {
        [cmd, rest] => (*cmd, rest.splitn(2, "=").collect::<Vec<_>>()),
        [cmd] => {
            return Err(HocLineParseError::new(format!(
                "command '{}' is missing arguments",
                cmd
            )));
        }
        _ => unreachable!(),
    };

    let (key, value) = match split.as_slice() {
        [key, value] => (*key, Some(parse_hoc_value(value)?.1)),
        [key] => (*key, None),
        _ => unreachable!(),
    };

    match (ns, cmd) {
        ("out", "set") | ("out", "append") => {
            let value = value.ok_or_else(|| {
                HocLineParseError::new(format!("missing value for command '{}'", cmd))
            })?;

            if cmd == "set" {
                output.insert(key.to_string(), value);
            } else {
                let existing = output.get_mut(key).ok_or_else(|| {
                    HocLineParseError::new(format!("uninitialized field '{}'", key))
                })?;

                match (existing, value) {
                    (HocValue::List(existing), value) => existing.push(value),
                    (HocValue::String(existing), HocValue::String(value)) => {
                        existing.push_str(&value)
                    }
                    (HocValue::String(_), _) => {
                        return Err(HocLineParseError::new(format!(
                            "expected string for key '{}'",
                            key
                        )))
                    }
                }
            }

            Ok(true)
        }

        ("in", "unset") => {
            input
                .remove(key)
                .ok_or_else(|| HocLineParseError::new(format!("'{}' is not defined", key)))?;
            Ok(true)
        }

        (ns, cmd) => Err(HocLineParseError::new(format!(
            "unknown command '{}' in namespace '{}'",
            cmd, ns
        ))),
    }
}

fn parse_hoc_value(mut s: &str) -> Result<(&str, HocValue), HocLineParseError> {
    const COMMA: char = ',';
    const DQUOTE: char = '"';
    const EMPTY_LIST: &str = "[]";
    const LBRACKET: char = '[';
    const RBRACKET: char = ']';
    const BSLASH: char = '\\';

    if s.starts_with(DQUOTE) {
        let mut chars = s
            .chars()
            .scan(0, |i, c| {
                let cur = *i;
                *i += c.len_utf8();
                Some((cur, c))
            })
            .skip(1);

        let mut value = String::new();

        while let Some((i, c)) = chars.next() {
            if c == BSLASH {
                if let Some((_, next_c)) = chars.next() {
                    if ![BSLASH, DQUOTE].contains(&next_c) {
                        return Err(HocLineParseError::new(format!(
                            "unknown character escape '{}'",
                            next_c
                        )));
                    } else {
                        value.push(next_c);
                    }
                } else {
                    return Err(HocLineParseError::new("trailing backslash"));
                }
            } else if c == DQUOTE {
                return Ok((&s[i + DQUOTE.len_utf8()..], HocValue::String(value)));
            } else {
                value.push(c);
            }
        }

        Err(HocLineParseError::new("unclosed double quote"))
    } else if s.starts_with(EMPTY_LIST) {
        Ok((&s[EMPTY_LIST.len()..], HocValue::List(Vec::new())))
    } else if s.starts_with(LBRACKET) {
        let mut v = Vec::new();

        s = &s[LBRACKET.len_utf8()..];
        loop {
            let (s_new, value) = parse_hoc_value(s)?;
            v.push(value);

            if s_new.starts_with(RBRACKET) {
                return Ok((&s_new[RBRACKET.len_utf8()..], HocValue::List(v)));
            } else if s_new.len() == 0 {
                return Err(HocLineParseError::new("unclosed bracket"));
            } else if !s_new.starts_with(COMMA) {
                return Err(HocLineParseError::new(format!(
                    "trailing characters '{}'",
                    s_new
                )));
            }

            s = &s_new[COMMA.len_utf8()..];
        }
    } else if s.len() == 0 {
        Err(HocLineParseError::new("empty value"))
    } else {
        Err(HocLineParseError::new(format!(
            "unexpected character '{}'",
            s.chars().next().unwrap()
        )))
    }
}
