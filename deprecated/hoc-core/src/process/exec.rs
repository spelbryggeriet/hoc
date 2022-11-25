use std::{borrow::Cow, fs::File, io::Write, process::Stdio};

use colored::Colorize;
use hoc_log::{info, status};

use super::{Error, ProcessOutput, Quotify, Settings};

pub const SUCCESS_CODE: i32 = 0;

pub fn exec(program: Cow<str>, args: Cow<str>, set: &Settings) -> Result<(i32, String), Error> {
    if !set.silent {
        let client = set.ssh_client.map_or_else(
            || "this computer".blue(),
            |client| client.options().host.as_deref().unwrap_or("unknown").blue(),
        );
        let sudo_str = set
            .sudo
            .is_some()
            .then(|| "sudo ".green().to_string())
            .unwrap_or_default();
        let program_str = program.green();
        let args_str = (!args.is_empty())
            .then(|| format!(" {args}"))
            .unwrap_or_default();

        let cmd_status = status!("Run ({client}): {sudo_str}{program_str}{args_str}",);

        match exec_impl(program, args, set) {
            Ok((status, output)) => {
                cmd_status
                    .with_label(format!("exit: {status}").green())
                    .finish();
                Ok((status, output))
            }

            Err(Error::Exit {
                program,
                status,
                stdout,
                stderr,
            }) => {
                cmd_status
                    .with_label(format!("exit: {status}").red())
                    .finish();
                Err(Error::Exit {
                    program,
                    status,
                    stdout,
                    stderr,
                })
            }

            Err(Error::Aborted { program }) => {
                cmd_status.with_label(format!("aborted").red()).finish();
                Err(Error::Aborted { program })
            }

            err => err,
        }
    } else {
        exec_impl(program, args, set)
    }
}

fn exec_impl(program: Cow<str>, args: Cow<str>, set: &Settings) -> Result<(i32, String), Error> {
    let mut pipe_input = set.pipe_input.clone();
    let mut cmd_str = String::new();
    if let Some((ref password, ref user)) = set.sudo {
        cmd_str += "sudo ";

        if let Some(password) = password {
            cmd_str += "-kSp '' ";
            pipe_input.insert(0, password.clone());
        } else {
            cmd_str += "-p ";
            cmd_str += &hoc_log::LOG
                .create_line_prefix("> [sudo] Password:")
                .quotify();
            cmd_str += " ";
        }

        if let Some(user) = user.as_ref() {
            cmd_str += "-H -u ";
            cmd_str += user;
            cmd_str += " "
        }

        if !set.env.is_empty() {
            cmd_str += "--preserve-env=";
            cmd_str += &set
                .env
                .keys()
                .map(Cow::as_ref)
                .collect::<Vec<_>>()
                .join(",");
            cmd_str += " ";
        }
    };

    cmd_str += &program;
    cmd_str += " ";
    cmd_str += &args;

    let (stdout, stderr, status) = if let Some(client) = set.ssh_client {
        if let Some(path) = set.stdout {
            cmd_str += &format!(" 1>{}", path.to_string_lossy().quotify());
        }

        let mut env_str = String::new();
        for (key, value) in set.env.iter() {
            env_str += &format!("{key}={} ", value.quotify());
        }
        cmd_str = format!("{env_str}{cmd_str}");

        let mut channel = client.spawn(&cmd_str, &pipe_input)?;

        (
            channel.read_stdout_to_string(!set.hide_stdout, &set.secrets)?,
            channel.read_stderr_to_string(!set.hide_stderr, &set.secrets)?,
            channel.finish()?,
        )
    } else {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", &cmd_str])
            .stdin(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(path) = set.stdout {
            cmd.stdout(
                File::options()
                    .write(true)
                    .truncate(true)
                    .create(true)
                    .open(path)?,
            );
        } else {
            cmd.stdout(Stdio::piped());
        }

        for (key, value) in set.env.iter() {
            cmd.env(key.as_ref(), value.as_ref());
        }

        let mut child = cmd.spawn()?;

        if !pipe_input.is_empty() {
            let mut stdin = child.stdin.take().unwrap();
            for input in &pipe_input {
                stdin.write_all(input.as_bytes())?;
                stdin.write_all(b"\n")?;
            }
        }

        (
            child.read_stdout_to_string(!set.hide_stdout, &set.secrets)?,
            child.read_stderr_to_string(!set.hide_stderr, &set.secrets)?,
            child.finish()?,
        )
    };

    if let Some(status) = status {
        let success_codes = set.success_codes.as_deref().unwrap_or(&[SUCCESS_CODE]);
        if success_codes.contains(&status) {
            if !set.silent {
                if set.hide_stdout && set.hide_stderr {
                    info!("{}", "<output hidden>".blue());
                } else if set.hide_stdout {
                    info!("{}", "<stdout hidden>".blue());
                } else if set.hide_stderr {
                    info!("{}", "<stderr hidden>".blue());
                }
            }

            Ok((status, stdout))
        } else {
            Err(Error::Exit {
                program: program.into_owned(),
                status,
                stdout,
                stderr,
            })
        }
    } else {
        Err(Error::Aborted {
            program: program.into_owned(),
        })
    }
}
