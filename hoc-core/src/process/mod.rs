use std::{
    borrow::Cow,
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, BufRead, BufReader, Read, Write},
    mem,
    process::{self, Stdio},
};

use colored::Colorize;
use hoc_log::{error, info, status};
use thiserror::Error;

#[doc(hidden)]
#[macro_export]
macro_rules! _with_dollar_sign {
    ($($body:tt)*) => {
        macro_rules! __with_dollar_sign { $($body)* }
        __with_dollar_sign!($);
    }
}

#[macro_export]
macro_rules! cmd {
    ($program:expr $(, $args:expr)* $(,)?) => {
        $crate::Process::cmd(&$program)
            $(.arg(&($args)))*
    };
}

pub mod ssh;

pub const SUCCESS_CODE: i32 = 0;

pub fn reset_sudo_privileges() -> Result<(), Error> {
    cmd!("sudo", "-k").silent().run().map(|_| ())
}

fn process_exit_err_msg(program: &str, status: i32, stdout: &str, stderr: &str) -> String {
    let output_str = if !stdout.is_empty() && !stderr.is_empty() {
        format!(":\n\n[stdout]\n{stdout}\n\n[stderr]\n{stderr}")
    } else if !stdout.is_empty() {
        format!(":\n\n[stdout]\n{stdout}")
    } else if !stderr.is_empty() {
        format!(":\n\n[stderr]\n{stderr}")
    } else {
        String::new()
    };

    format!("{program} exited with status code {status}{output_str}")
}

trait Obfuscate<'a> {
    fn obfuscate(self, secrets: &[&str]) -> Cow<'a, str>;
}

impl<'a> Obfuscate<'a> for Cow<'a, str> {
    fn obfuscate(mut self, secrets: &[&str]) -> Cow<'a, str> {
        for secret in secrets {
            if self.contains(secret) {
                self = Cow::Owned(self.replace(secret, &"<obfuscated>".red().to_string()));
            }
        }
        self
    }
}

impl<'a> Obfuscate<'a> for String {
    fn obfuscate(self, secrets: &[&str]) -> Cow<'a, str> {
        Cow::<str>::Owned(self).obfuscate(secrets)
    }
}

impl<'a> Obfuscate<'a> for &'a str {
    fn obfuscate(self, secrets: &[&str]) -> Cow<'a, str> {
        Cow::Borrowed(self).obfuscate(secrets)
    }
}

trait Quotify<'a> {
    fn needs_quotes(&self) -> bool;
    fn quotify(self) -> Cow<'a, str>;
}

impl<'a> Quotify<'a> for Cow<'a, str> {
    fn needs_quotes(&self) -> bool {
        self.is_empty()
            || self
                .chars()
                .any(|c| c.is_whitespace() || c == '$' || c == '`')
    }

    fn quotify(self) -> Cow<'a, str> {
        if self.needs_quotes() {
            Cow::Owned(format!("'{}'", self.replace("'", r"'\''")))
        } else {
            self
        }
    }
}

impl<'a> Quotify<'a> for String {
    fn needs_quotes(&self) -> bool {
        Cow::needs_quotes(&Cow::Borrowed(self))
    }

    fn quotify(self) -> Cow<'a, str> {
        Cow::quotify(Cow::Owned(self))
    }
}

impl<'a> Quotify<'a> for &'a str {
    fn needs_quotes(&self) -> bool {
        Cow::needs_quotes(&Cow::Borrowed(self))
    }

    fn quotify(self) -> Cow<'a, str> {
        Cow::quotify(Cow::Borrowed(self))
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] io::Error),

    #[error("ssh: {0}")]
    Ssh(#[from] ssh::SshError),

    #[error("{}", process_exit_err_msg(program, *status, stdout, stderr))]
    Exit {
        program: String,
        status: i32,
        stdout: String,
        stderr: String,
    },

    #[error("{program} was aborted")]
    Aborted { program: String },
}

impl From<Error> for hoc_log::Error {
    fn from(err: Error) -> Self {
        error!("{err}").unwrap_err()
    }
}

pub struct Process<'a> {
    program: &'a OsStr,
    args: Vec<Cow<'a, OsStr>>,
    ssh_client: Option<&'a ssh::SshClient>,
    sudo: Option<Option<Cow<'a, str>>>,
    pipe_input: Vec<Cow<'a, str>>,
    stdout: Option<&'a OsStr>,
    secrets: Vec<&'a str>,
    success_codes: Option<Vec<i32>>,
    silent: bool,
    hide_stdout: bool,
    hide_stderr: bool,
}

impl<'process> Process<'process> {
    pub fn cmd<S: AsRef<OsStr>>(program: &'process S) -> Self {
        Self {
            program: program.as_ref(),
            args: Vec::new(),
            sudo: None,
            ssh_client: None,
            pipe_input: Vec::new(),
            stdout: None,
            secrets: Vec::new(),
            success_codes: None,
            silent: false,
            hide_stdout: false,
            hide_stderr: false,
        }
    }

    pub fn arg<S: AsRef<OsStr>>(mut self, arg: &'process S) -> Self {
        self.args.push(Cow::Borrowed(arg.as_ref()));
        self
    }

    pub fn ssh(mut self, client: &'process ssh::SshClient) -> Self {
        self.ssh_client = Some(client);
        self
    }

    pub fn sudo(mut self) -> Self {
        self.sudo = Some(None);
        self
    }

    pub fn sudo_password<S: Into<Cow<'process, str>>>(mut self, password: S) -> Self {
        self.sudo = Some(Some(password.into()));
        self
    }

    pub fn stdin_line<S: Into<Cow<'process, str>>>(mut self, input: S) -> Self {
        self.pipe_input.push(input.into());
        self
    }

    pub fn stdin_lines<I, S>(mut self, input: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<Cow<'process, str>>,
    {
        self.pipe_input.extend(input.into_iter().map(Into::into));
        self
    }

    pub fn secret<S: AsRef<str>>(mut self, secret: &'process S) -> Self {
        self.secrets.push(secret.as_ref());
        self
    }

    pub fn success_codes<I: IntoIterator<Item = i32>>(mut self, input: I) -> Self {
        let iter = input.into_iter();
        if let Some(success_codes) = self.success_codes.as_mut() {
            success_codes.extend(iter)
        } else {
            self.success_codes = Some(iter.collect())
        }
        self
    }

    pub fn silent(mut self) -> Self {
        self.silent = true;
        self
    }

    pub fn stdout<S: AsRef<OsStr>>(mut self, path: &'process S) -> Self {
        self.stdout = Some(path.as_ref());
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

    pub fn run(self) -> Result<(i32, String), Error> {
        let show_stdout = !self.silent && !self.hide_stdout;
        let show_stderr = !self.silent && !self.hide_stderr;

        if !self.silent {
            let sudo_str = if self.sudo.is_some() {
                "sudo ".green().to_string()
            } else {
                String::new()
            };
            let command_str = self
                .args
                .iter()
                .map(|arg| {
                    let arg = arg.to_string_lossy().obfuscate(&self.secrets);
                    if arg.needs_quotes() {
                        Cow::Owned(arg.quotify().yellow().to_string())
                    } else {
                        arg.quotify()
                    }
                })
                .fold(
                    self.program.to_string_lossy().green().to_string(),
                    |out, arg| out + " " + &arg,
                );
            let redirect_output_str = if let Some(path) = self.stdout {
                format!(" 1>{}", path.to_string_lossy().quotify())
                    .blue()
                    .to_string()
            } else {
                String::new()
            };
            let redirect_input_str = if !self.pipe_input.is_empty() {
                format!(" {}{}", "0<".blue(), "'mark'".obfuscate(&["mark"]).yellow())
            } else {
                String::new()
            };

            let client = if let Some(ref client) = self.ssh_client {
                client.host().blue()
            } else {
                "this computer".blue()
            };

            let cmd_status = status!(
                "Run command on {client}: {sudo_str}{command_str}{redirect_output_str}{redirect_input_str}",
            );

            match Process::exec(self, show_stdout, show_stderr) {
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
            Process::exec(self, show_stdout, show_stderr)
        }
    }

    fn exec(mut self, show_stdout: bool, show_stderr: bool) -> Result<(i32, String), Error> {
        let program_str = self.program.to_string_lossy().into_owned();

        if let Some(sudo) = self.sudo {
            self.args.insert(
                0,
                Cow::Borrowed(mem::replace(&mut self.program, OsStr::new("sudo"))),
            );

            if let Some(password) = sudo {
                self.args.insert(0, Cow::Borrowed(OsStr::new("-kSp")));
                self.args.insert(1, Cow::Borrowed(OsStr::new("")));
                let mut pipe_input = vec![password];
                pipe_input.extend(self.pipe_input);
                self.pipe_input = pipe_input;
            } else {
                let line_prefix =
                    OsString::from(hoc_log::LOG.create_line_prefix("[sudo] Password:"));
                self.args.insert(0, Cow::Borrowed(OsStr::new("-p")));
                self.args.insert(1, Cow::Owned(line_prefix));
            }
        };

        let (stdout, stderr, status) = if let Some(client) = self.ssh_client {
            let mut cmd = self
                .args
                .iter()
                .map(|arg| arg.to_string_lossy().quotify())
                .fold(self.program.to_string_lossy().into_owned(), |out, arg| {
                    out + " " + &arg
                });

            if let Some(path) = self.stdout {
                cmd += &format!(" 1>{}", path.to_string_lossy().quotify());
            }

            let mut channel = client.spawn(&cmd, &self.pipe_input)?;

            (
                channel.read_stdout_to_string(show_stdout, &self.secrets)?,
                channel.read_stderr_to_string(show_stderr, &self.secrets)?,
                channel.finish()?,
            )
        } else {
            let mut cmd = process::Command::new(self.program);
            cmd.args(&self.args)
                .stdin(Stdio::piped())
                .stderr(Stdio::piped());

            if let Some(path) = self.stdout {
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

            if !self.pipe_input.is_empty() {
                let mut stdin = child.stdin.take().unwrap();
                for input in &self.pipe_input {
                    stdin.write_all(input.as_bytes())?;
                    stdin.write_all(b"\n")?;
                }
            }

            (
                child.read_stdout_to_string(show_stdout, &self.secrets)?,
                child.read_stderr_to_string(show_stderr, &self.secrets)?,
                child.finish()?,
            )
        };

        if let Some(status) = status {
            let success_codes = self.success_codes.as_deref().unwrap_or(&[SUCCESS_CODE]);
            if success_codes.contains(&status) {
                if !self.silent {
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
}

trait ProcessOutput {
    type Stdout: Read;
    type Stderr: Read;

    fn stdout(&mut self) -> Self::Stdout;
    fn stderr(&mut self) -> Self::Stderr;
    fn finish(self) -> Result<Option<i32>, Error>;

    fn read_stdout_to_string(
        &mut self,
        show_output: bool,
        secrets: &[&str],
    ) -> Result<String, Error> {
        Self::read_lines(self.stdout(), show_output, secrets)
    }

    fn read_stderr_to_string(
        &mut self,
        show_output: bool,
        secrets: &[&str],
    ) -> Result<String, Error> {
        Self::read_lines(self.stderr(), show_output, secrets)
    }

    fn read_lines(reader: impl Read, show_output: bool, secrets: &[&str]) -> Result<String, Error> {
        let mut output = String::new();
        let buf_reader = BufReader::new(reader);
        for line in buf_reader.lines() {
            let line = line?;

            if show_output {
                info!("{}", line.as_str().obfuscate(secrets));
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

    fn finish(mut self) -> Result<Option<i32>, Error> {
        let status = self.wait()?;
        Ok(status.code())
    }
}
