use std::{
    borrow::Cow,
    ffi::{OsStr, OsString},
    fs::File,
    io::Write,
    mem,
    process::Stdio,
};

use colored::Colorize;
use hoc_log::{info, status};

use super::{Error, Obfuscate, ProcessOutput, Quotify, Settings};

pub const SUCCESS_CODE: i32 = 0;

pub fn exec(
    program: &OsStr,
    args: Vec<Cow<OsStr>>,
    set: &Settings,
) -> Result<(i32, String), Error> {
    if !set.silent {
        let sudo_str = if set.sudo.is_some() {
            "sudo ".green().to_string()
        } else {
            String::new()
        };

        let command_str = args
            .iter()
            .map(|arg| {
                let arg = arg.to_string_lossy().obfuscate(&set.secrets);
                if arg.needs_quotes() {
                    Cow::Owned(arg.quotify().yellow().to_string())
                } else {
                    arg.quotify()
                }
            })
            .fold(program.to_string_lossy().green().to_string(), |out, arg| {
                out + " " + &arg
            });

        let client = if let Some(ref client) = set.ssh_client {
            client.options().host.as_deref().unwrap_or("unknown").blue()
        } else {
            "this computer".blue()
        };

        let cmd_status = status!("Run ({client}): {sudo_str}{command_str}",);

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

fn exec_impl<'args, 'a: 'args>(
    mut program: &'a OsStr,
    mut args: Vec<Cow<'args, OsStr>>,
    set: &'a Settings,
) -> Result<(i32, String), Error> {
    let program_str = program.to_string_lossy().into_owned();
    let mut pipe_input = set.pipe_input.clone();

    if let Some((ref password, ref user)) = set.sudo {
        args.insert(
            0,
            Cow::Borrowed(mem::replace(&mut program, OsStr::new("sudo"))),
        );

        if let Some(user) = user {
            args.insert(0, Cow::Borrowed(OsStr::new("-H")));
            args.insert(1, Cow::Borrowed(OsStr::new("-u")));
            args.insert(2, Cow::Borrowed(user));
        }

        if let Some(password) = password {
            args.insert(3, Cow::Borrowed(OsStr::new("-kSp")));
            args.insert(4, Cow::Borrowed(OsStr::new("")));
            pipe_input.insert(0, password.clone());
        } else {
            let line_prefix = OsString::from(hoc_log::LOG.create_line_prefix("[sudo] Password:"));
            args.insert(3, Cow::Borrowed(OsStr::new("-p")));
            args.insert(4, Cow::Owned(line_prefix));
        }
    };

    let mut cmd_str = args
        .iter()
        .map(|arg| arg.to_string_lossy().quotify())
        .fold(program.to_string_lossy().into_owned(), |out, arg| {
            out + " " + &arg
        });

    let (stdout, stderr, status) = if let Some(client) = set.ssh_client {
        if let Some(path) = set.stdout {
            cmd_str += &format!(" 1>{}", path.to_string_lossy().quotify());
        }

        let mut env_str = String::new();
        for (key, value) in set.env.iter() {
            env_str += &format!("export {key}={} ; ", value.quotify());
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
                program: program_str,
                status,
                stdout,
                stderr,
            })
        }
    } else {
        Err(Error::Aborted {
            program: program_str,
        })
    }
}
