use std::{
    borrow::Cow,
    ffi::{OsStr, OsString},
    io::{self, BufRead, BufReader, Read, Write},
    mem,
    num::NonZeroI32,
    process::{self, Stdio},
};

use hoclog::{error, info, status};
use thiserror::Error;

use crate::StdResult;

macro_rules! cmd {
    ($program:expr $(, $args:expr)* $(,)?) => {
        $crate::command::util::Process::cmd($program)
            $(.arg(&($args)))*
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
    hide_output: bool,
}

impl<'ssh> Process<'ssh> {
    pub fn cmd<S: AsRef<OsStr>>(program: S) -> Self {
        Self {
            program: program.as_ref().to_os_string(),
            args: Vec::new(),
            sudo: None,
            ssh_client: None,
            silent: false,
            hide_output: false,
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

    pub fn hide_output(mut self) -> Self {
        self.hide_output = true;
        self
    }

    pub fn run(self) -> Result<String> {
        let show_output = !self.silent && !self.hide_output;

        let output = if !self.silent {
            let args_iter = if self.sudo.is_some() {
                self.args.iter().skip(2)
            } else {
                self.args.iter().skip(0)
            };

            let sudo_str = if self.sudo.is_some() { "sudo " } else { "" };
            let command_string = format!(
                "{}{} {}",
                sudo_str,
                self.program.to_string_lossy(),
                args_iter
                    .map(|a| a.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" "),
            );

            let client = if let Some(ref client) = self.ssh_client {
                client.host()
            } else {
                "this computer"
            };

            if !self.hide_output {
                status!("Running command on {}: {}", client, command_string => {
                    self.exec(show_output)?
                })
            } else {
                info!("Running command on {}: {}", client, command_string);
                self.exec(show_output)?
            }
        } else {
            self.exec(show_output)?
        };

        Ok(output)
    }

    fn exec(mut self, show_output: bool) -> Result<String> {
        let program_str = if self.sudo.is_some() {
            self.args[2].to_string_lossy().into_owned()
        } else {
            self.program.to_string_lossy().into_owned()
        };

        let pipe_input = if let Some(ref sudo) = self.sudo {
            self.args
                .insert(0, mem::replace(&mut self.program, OsString::from("sudo")));

            if let Some(ref password) = sudo {
                self.args.insert(0, OsString::from("-kSp"));
                self.args.insert(1, OsString::new());
                Some(password.as_ref())
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
            let mut channel = client.spawn(&cmd, pipe_input)?;

            (
                channel.read_stdout_to_string(show_output)?,
                channel.read_stderr_to_string()?,
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
                child.stdin.take().unwrap().write_all(pipe_input)?;
            }

            (
                child.read_stdout_to_string(show_output)?,
                child.read_stderr_to_string()?,
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
        if show_output {
            Self::read_lines(self.stdout(), |line| info!(line))
        } else {
            Self::read_lines(self.stdout(), |_| ())
        }
    }

    fn read_stderr_to_string(&mut self) -> Result<String> {
        Self::read_lines(self.stderr(), |_| ())
    }

    fn read_lines(reader: impl Read, for_each_line: impl Fn(&str)) -> Result<String> {
        let mut output = String::new();
        let buf_reader = BufReader::new(reader);
        for line in buf_reader.lines() {
            let line = line?;
            for_each_line(&line);

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
