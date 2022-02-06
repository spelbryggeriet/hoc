use std::{
    borrow::Cow,
    ffi::{OsStr, OsString},
    io::{self, BufRead, BufReader, Read, Write},
    mem,
    num::NonZeroI32,
    process::{self, Stdio},
};

use colored::Colorize;
use hoclog::{error, info, status};
use thiserror::Error;

use crate::StdResult;

macro_rules! _with_dollar_sign {
    ($($body:tt)*) => {
        macro_rules! __with_dollar_sign { $($body)* }
        __with_dollar_sign!($);
    }
}

macro_rules! cmd {
    ($program:expr $(, $args:expr)* $(,)?) => {
        $crate::command::util::Process::cmd($program)
            $(.arg(&($args)))*
    };
}

macro_rules! cmd_template {
    ($($name:ident => $program:literal $(, $args:tt)* $(,)?);* $(;)?) => {
        _with_dollar_sign!(($d:tt) => {
            $(cmd_template!(@impl $d, $name => [$($args,)*] => [$program,] => []);)*
        });
    };

    (@impl $d:tt, $name:ident => [$arg:literal, $($args:tt,)*] => [$($cmd:tt)*] => [$($idents:tt)*]) => {
        cmd_template!(@impl $d, $name => [$($args,)*] => [$($cmd)* $arg,] => [$($idents)*]);
    };

    (@impl $d:tt, $name:ident => [$arg:ident, $($args:tt,)*] => [$($cmd:tt)*] => [$($idents:tt)*]) => {
        cmd_template!(@impl $d, $name => [$($args,)*] => [$($cmd)* $d $arg,] => [$($idents)* $arg]);
    };

    (@impl $d:tt, $name:ident => [($tmpl:literal $(, $arg:ident)* $(,)?), $($args:tt,)*] => [$($cmd:tt)*] => [$($idents:tt)*]) => {
        cmd_template!(@impl $d, $name => [$($args,)*] => [$($cmd)* format!($tmpl, $( $d $arg,)*),] => [$($idents)* $($arg)*]);
    };

    (@impl $d:tt, $name:ident => [] => [$($cmd:tt)*] => [$($idents:tt)*]) => {
        macro_rules! $name {
            ($($d $idents:expr),* $d (,)?) => {
                cmd!($($cmd)*)
            };
        }
    };
}

pub mod ssh;

pub fn reset_sudo_privileges() -> Result<()> {
    cmd!("sudo", "-k").silent().run().map(|_| ())
}

type Result<T> = StdResult<T, ProcessError>;

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error(transparent)]
    Io(#[from] io::Error),

    #[error("ssh: {0}")]
    Ssh(#[from] ssh::Error),

    #[error("{program} failed: status code {status}\n\n[stdout]\n{stdout}\n[stderr]\n{stderr}")]
    Exit {
        program: String,
        status: NonZeroI32,
        stdout: String,
        stderr: String,
    },
}

pub struct Process<'ssh> {
    program: OsString,
    args: Vec<OsString>,
    sudo: Option<Option<Vec<u8>>>,
    ssh_client: Option<&'ssh ssh::Client>,
    silent: bool,
    hide_stdout: bool,
    hide_stderr: bool,
    pipe_input: Option<Vec<Vec<u8>>>,
}

impl<'ssh> Process<'ssh> {
    pub fn cmd<S: AsRef<OsStr>>(program: S) -> Self {
        Self {
            program: program.as_ref().to_os_string(),
            args: Vec::new(),
            sudo: None,
            ssh_client: None,
            silent: false,
            hide_stdout: false,
            hide_stderr: false,
            pipe_input: None,
        }
    }

    pub fn arg<S: Into<OsString>>(mut self, arg: S) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn sudo(mut self) -> Self {
        self.sudo = Some(None);
        self
    }

    pub fn sudo_password<B: Into<Vec<u8>>>(mut self, password: B) -> Self {
        self.sudo = Some(Some(password.into()));
        self
    }

    pub fn ssh(mut self, client: &'ssh ssh::Client) -> Self {
        self.ssh_client = Some(client);
        self
    }

    pub fn silent(mut self) -> Self {
        self.silent = true;
        self
    }

    pub fn hide_stdout(mut self) -> Self {
        self.hide_stdout = true;
        self
    }

    pub fn hide_stderr(mut self) -> Self {
        self.hide_stderr = true;
        self
    }

    pub fn hide_output(self) -> Self {
        self.hide_stdout().hide_stderr()
    }

    pub fn pipe_input<I, B>(mut self, input: I) -> Self
    where
        I: IntoIterator<Item = B>,
        B: Into<Vec<u8>>,
    {
        let iter = input.into_iter().map(Into::into);
        if let Some(pipe_input) = self.pipe_input.as_mut() {
            pipe_input.extend(iter)
        } else {
            self.pipe_input = Some(iter.collect())
        }
        self
    }

    pub fn run(self) -> Result<String> {
        let show_stdout = !self.silent && !self.hide_stdout;
        let show_stderr = !self.silent && !self.hide_stderr;

        let output = if !self.silent {
            let args_iter = if self.sudo.is_some() {
                self.args.iter().skip(2)
            } else {
                self.args.iter().skip(0)
            };

            let sudo_str = if self.sudo.is_some() { "sudo " } else { "" };
            let command_string = format!(
                "{}{} {}",
                sudo_str.green(),
                self.program.to_string_lossy().green(),
                args_iter
                    .map(|a| a.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" "),
            );

            let client = if let Some(ref client) = self.ssh_client {
                client.host().blue()
            } else {
                "this computer".blue()
            };

            if !self.hide_stdout || !self.hide_stderr {
                status!("Running command on {}: {}", client, command_string => {
                    self.exec(show_stdout, show_stderr)?
                })
            } else {
                info!("Running command on {}: {}", client, command_string);
                self.exec(show_stdout, show_stderr)?
            }
        } else {
            self.exec(show_stdout, show_stderr)?
        };

        Ok(output)
    }

    fn exec(mut self, show_stdout: bool, show_stderr: bool) -> Result<String> {
        let program_str = self.program.to_string_lossy().into_owned();

        let mut pipe_input = if let Some(sudo) = self.sudo {
            self.args
                .insert(0, mem::replace(&mut self.program, OsString::from("sudo")));

            if let Some(password) = sudo {
                self.args.insert(0, OsString::from("-kSp"));
                self.args.insert(1, OsString::new());
                Some(vec![password])
            } else {
                self.args.insert(0, OsString::from("-p"));
                self.args.insert(
                    1,
                    OsString::from(hoclog::LOG.create_line_prefix("Password:")),
                );
                None
            }
        } else {
            None
        };

        if let Some(input) = self.pipe_input {
            if let Some(pipe_input) = pipe_input.as_mut() {
                pipe_input.extend(input);
            } else {
                pipe_input = Some(input);
            }
        }

        let (stdout, stderr, status) = if let Some(client) = self.ssh_client {
            let cmd = self
                .args
                .iter()
                .map(|arg| {
                    let arg = arg.to_string_lossy();
                    if arg.is_empty() || arg.chars().any(char::is_whitespace) {
                        Cow::Owned(format!("'{}'", arg))
                    } else {
                        arg
                    }
                })
                .fold(self.program.to_string_lossy().into_owned(), |out, arg| {
                    out + " " + &arg
                });
            let mut channel = client.spawn(&cmd, pipe_input.as_deref())?;

            (
                channel.read_stdout_to_string(show_stdout)?,
                channel.read_stderr_to_string(show_stderr)?,
                channel.finish()?,
            )
        } else {
            let mut child = process::Command::new(self.program)
                .args(self.args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;

            if let Some(pipe_input) = pipe_input {
                let mut stdin = child.stdin.take().unwrap();
                for input in pipe_input {
                    stdin.write_all(&input)?;
                    stdin.write_all(b"\n")?;
                }
            }

            (
                child.read_stdout_to_string(show_stdout)?,
                child.read_stderr_to_string(show_stderr)?,
                child.finish()?,
            )
        };

        if let Some(status) = status {
            if status != 0 {
                return Err(ProcessError::Exit {
                    program: program_str,
                    status: unsafe { NonZeroI32::new_unchecked(status) },
                    stdout,
                    stderr,
                });
            }
        }

        Ok(stdout)
    }
}

trait ProcessOutput {
    type Stdout: Read;
    type Stderr: Read;

    fn stdout(&mut self) -> Self::Stdout;
    fn stderr(&mut self) -> Self::Stderr;
    fn finish(self) -> Result<Option<i32>>;

    fn read_stdout_to_string(&mut self, show_output: bool) -> Result<String> {
        Self::read_lines(self.stdout(), show_output)
    }

    fn read_stderr_to_string(&mut self, show_output: bool) -> Result<String> {
        Self::read_lines(self.stderr(), show_output)
    }

    fn read_lines(reader: impl Read, show_output: bool) -> Result<String> {
        let mut output = String::new();
        let buf_reader = BufReader::new(reader);
        for line in buf_reader.lines() {
            let line = line?;
            if show_output {
                info!(&line);
            }

            let is_empty = output.is_empty();
            if !is_empty {
                output.push('\n');
            }

            if !is_empty || !line.trim().is_empty() {
                output.push_str(&line);
            }
        }
        output.truncate(output.trim_end().len());

        Ok(output)
    }
}

impl ProcessOutput for process::Child {
    type Stdout = process::ChildStdout;
    type Stderr = process::ChildStderr;

    fn stdout(&mut self) -> Self::Stdout {
        self.stdout.take().unwrap()
    }

    fn stderr(&mut self) -> Self::Stderr {
        self.stderr.take().unwrap()
    }

    fn finish(mut self) -> Result<Option<i32>> {
        let status = self.wait()?;
        Ok(status.code())
    }
}
