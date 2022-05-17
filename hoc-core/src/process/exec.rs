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

pub fn exec<'a>(
    program: &'a OsStr,
    args: Vec<Cow<'a, OsStr>>,
    set: &Settings,
) -> Result<(i32, String), Error> {
    let show_stdout = !set.silent && !set.hide_stdout;
    let show_stderr = !set.silent && !set.hide_stderr;

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
        let redirect_output_str = if let Some(path) = set.stdout {
            format!(" 1>{}", path.to_string_lossy().quotify())
                .blue()
                .to_string()
        } else {
            String::new()
        };
        let redirect_input_str = if !set.pipe_input.is_empty() {
            format!(" {}{}", "0<".blue(), "'mark'".obfuscate(&["mark"]).yellow())
        } else {
            String::new()
        };

        let client = if let Some(ref client) = set.ssh_client {
            client.host().blue()
        } else {
            "this computer".blue()
        };

        let cmd_status = status!(
                "Run command on {client}: {sudo_str}{command_str}{redirect_output_str}{redirect_input_str}",
            );

        match exec_impl(program, args, show_stdout, show_stderr, set) {
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
        exec_impl(program, args, show_stdout, show_stderr, set)
    }
}

fn exec_impl<'a>(
    mut program: &'a OsStr,
    mut args: Vec<Cow<'a, OsStr>>,
    show_stdout: bool,
    show_stderr: bool,
    set: &Settings,
) -> Result<(i32, String), Error> {
    let program_str = program.to_string_lossy().into_owned();
    let mut pipe_input = set.pipe_input.clone();

    if let Some(ref sudo) = set.sudo {
        args.insert(
            0,
            Cow::Borrowed(mem::replace(&mut program, OsStr::new("sudo"))),
        );

        if let Some(password) = sudo {
            args.insert(0, Cow::Borrowed(OsStr::new("-kSp")));
            args.insert(1, Cow::Borrowed(OsStr::new("")));
            pipe_input.insert(0, password.clone());
        } else {
            let line_prefix = OsString::from(hoc_log::LOG.create_line_prefix("[sudo] Password:"));
            args.insert(0, Cow::Borrowed(OsStr::new("-p")));
            args.insert(1, Cow::Owned(line_prefix));
        }
    };

    let (stdout, stderr, status) = if let Some(client) = set.ssh_client {
        let mut cmd = args
            .iter()
            .map(|arg| arg.to_string_lossy().quotify())
            .fold(program_str.clone(), |out, arg| out + " " + &arg);

        if let Some(ref working_directory) = set.working_directory {
            cmd = format!("cd {} ; {}", working_directory, cmd);
        }

        if let Some(path) = set.stdout {
            cmd += &format!(" 1>{}", path.to_string_lossy().quotify());
        }

        let mut channel = client.spawn(&cmd, &set.pipe_input)?;

        (
            channel.read_stderr_to_string(show_stdout, &set.secrets)?,
            channel.read_stderr_to_string(show_stderr, &set.secrets)?,
            channel.finish()?,
        )
    } else {
        let mut cmd = std::process::Command::new(program);
        cmd.args(&args).stdin(Stdio::piped()).stderr(Stdio::piped());

        if let Some(ref working_directory) = set.working_directory {
            cmd.current_dir(working_directory.as_ref());
        }

        if let Some(path) = set.stdout {
            cmd.stdin(
                File::options()
                    .write(true)
                    .truncate(true)
                    .create(true)
                    .open(path)?,
            );
        } else {
            cmd.stdout(Stdio::piped());
        }

        let mut child = cmd.spawn()?;

        if !set.pipe_input.is_empty() {
            let mut stdin = child.stdin.take().unwrap();
            for input in &set.pipe_input {
                stdin.write_all(input.as_bytes())?;
                stdin.write_all(b"\n")?;
            }
        }

        (
            child.read_stdout_to_string(show_stdout, &set.secrets)?,
            child.read_stderr_to_string(show_stderr, &set.secrets)?,
            child.finish()?,
        )
    };

    if let Some(status) = status {
        let success_codes = set.success_codes.as_deref().unwrap_or(&[SUCCESS_CODE]);
        if success_codes.contains(&status) {
            if !set.silent {
                if !show_stdout && !show_stderr {
                    info!("{}", "<output hidden>".blue());
                } else if !show_stdout {
                    info!("{}", "<stdout hidden>".blue());
                } else if !show_stderr {
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
