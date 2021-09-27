mod parse;

use std::{collections::VecDeque, path::Path};

use crate::{HocState, HocValue};
use heck::SnakeCase;
use hoclog::Log;
use parse::HocLineParseError;

const VALUE: &str = "value";
const STRING: &str = "string";
const LIST: &str = "list";

const EMPTY_OUTPUT: &str = "";

const COMMANDS: &[(&str, &[(&str, &[Option<&str>])])] = &[
    ("do", &[("snake_case", &[Some(STRING)])]),
    (
        "in",
        &[
            ("choose", &[Some(STRING), Some(LIST)]),
            ("hidden_input", &[Some(STRING)]),
            ("prompt", &[Some(STRING)]),
            ("unset", &[None]),
        ],
    ),
    (
        "out",
        &[
            ("append", &[Some(VALUE)]),
            ("set", &[Some(VALUE)]),
            ("static", &[Some(VALUE)]),
        ],
    ),
    ("state", &[("persist", &[Some(VALUE)]), ("forget", &[None])]),
];

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

fn write_to_pipe(sync_pipe: &Path, contents: impl AsRef<[u8]>) -> Result<(), HocLineParseError> {
    std::fs::write(sync_pipe, contents)
        .map_err(|e| HocLineParseError::new(format!("Failed to write to sync pipe: {}", e)))
}

pub fn exec_hoc_line(
    log: &Log,
    input: &mut HocState,
    output: &mut HocState,
    static_keys: &mut Vec<String>,
    state: &mut HocState,
    sync_pipe: &Path,
    line: &str,
) -> Result<Option<(&'static str, &'static str)>, HocLineParseError> {
    let (ns, cmd, mut args) = if let Some(r) = parse::parse_hoc_line(line)? {
        r
    } else {
        return Ok(None);
    };

    let prefix = format!("executing command '{}' in namespace '{}'", cmd, ns);

    let sync_pipe_output = match (ns, cmd) {
        ("do", "snake_case") => {
            let string = args
                .pop_string_for_key_checked("string")
                .map_err(|err| HocLineParseError::new(format!("{}: {}", prefix, err)))?;

            string.to_snake_case()
        }

        ("in", "choose") => {
            let prompt = args
                .pop_string_for_key_checked("text")
                .map_err(|err| HocLineParseError::new(format!("{}: {}", prefix, err)))?;
            let options = args
                .pop_list_for_key_checked("options")
                .map_err(|err| HocLineParseError::new(format!("{}: {}", prefix, err)))?
                .into_iter()
                .map(|v| v.as_string().ok())
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| HocLineParseError::new("expected options type be of type string"))?;

            log.choose(prompt, options, 0).unwrap().to_string()
        }

        ("in", "hidden_input") => {
            let prompt = args
                .pop_string_for_key_checked("text")
                .map_err(|err| HocLineParseError::new(format!("{}: {}", prefix, err)))?;

            log.hidden_input(prompt)
        }

        ("in", "prompt") => {
            let prompt = args
                .pop_string_for_key_checked("text")
                .map_err(|err| HocLineParseError::new(format!("{}: {}", prefix, err)))?;

            (log.prompt(prompt) as u32).to_string()
        }

        ("in", "unset") => {
            let key = args.pop_key();

            input.remove(key).ok_or_else(|| {
                HocLineParseError::new(format!("{}: '{}' is not defined", prefix, key))
            })?;

            EMPTY_OUTPUT.to_string()
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
                    existing.push_str(
                        &args.pop_string_for_key_checked(key).map_err(|err| {
                            HocLineParseError::new(format!("{}: {}", prefix, err))
                        })?,
                    );
                }
            }

            EMPTY_OUTPUT.to_string()
        }

        ("out", "set") => {
            let (key, value) = args.pop_key_value();
            output.insert(key.to_string(), value);

            EMPTY_OUTPUT.to_string()
        }

        ("out", "static") => {
            let (key, value) = args.pop_key_value();
            output.insert(key.to_string(), value);
            static_keys.push(key.to_string());

            EMPTY_OUTPUT.to_string()
        }

        ("state", "persist") => {
            let (key, value) = args.pop_key_value();
            state.insert(key.to_string(), value);

            EMPTY_OUTPUT.to_string()
        }

        ("state", "forget") => {
            let key = args.pop_key();
            state.remove(key);

            EMPTY_OUTPUT.to_string()
        }

        (ns, cmd) => unreachable!("did not expect command '{}' in namespace '{}'", cmd, ns),
    };

    write_to_pipe(sync_pipe, sync_pipe_output)?;

    Ok(Some((ns, cmd)))
}
