use std::{
    borrow::Cow,
    ffi::OsStr,
    io::{self, BufRead, BufReader, Read},
    process,
};

use colored::Colorize;
use hoc_log::{error, info};
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
        $crate::process::Process::cmd(&$program)
            $(.arg(&($args)))*
    };
}

mod exec;
pub mod ssh;

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
    settings: Settings<'a>,
}

impl<'process> Process<'process> {
    pub fn cmd<S: AsRef<OsStr>>(program: &'process S) -> Self {
        Self {
            program: program.as_ref(),
            args: Vec::new(),
            settings: Settings::default(),
        }
    }

    pub fn arg<S: AsRef<OsStr>>(mut self, arg: &'process S) -> Self {
        self.args.push(Cow::Borrowed(arg.as_ref()));
        self
    }

    pub fn settings(mut self, settings: &'process Settings) -> Self {
        self.settings = Settings::from_settings(settings);
        self
    }

    pub fn ssh(mut self, client: &'process ssh::SshClient) -> Self {
        self.settings = self.settings.ssh(client);
        self
    }

    pub fn working_directory<S: Into<Cow<'process, str>>>(mut self, working_directory: S) -> Self {
        self.settings = self.settings.working_directory(working_directory);
        self
    }

    pub fn sudo(mut self) -> Self {
        self.settings = self.settings.sudo();
        self
    }

    pub fn sudo_password<S: Into<Cow<'process, str>>>(mut self, password: S) -> Self {
        self.settings = self.settings.sudo_password(password);
        self
    }

    pub fn stdin_line<S: Into<Cow<'process, str>>>(mut self, input: S) -> Self {
        self.settings = self.settings.stdin_line(input);
        self
    }

    pub fn stdin_lines<I, S>(mut self, input: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<Cow<'process, str>>,
    {
        self.settings = self.settings.stdin_lines(input);
        self
    }

    pub fn secret<S: AsRef<str>>(mut self, secret: &'process S) -> Self {
        self.settings = self.settings.secret(secret);
        self
    }

    pub fn success_codes<I: IntoIterator<Item = i32>>(mut self, input: I) -> Self {
        self.settings = self.settings.success_codes(input);
        self
    }

    pub fn silent(mut self) -> Self {
        self.settings = self.settings.silent();
        self
    }

    pub fn stdout<S: AsRef<OsStr>>(mut self, path: &'process S) -> Self {
        self.settings = self.settings.stdout(path);
        self
    }

    pub fn hide_stdout(mut self) -> Self {
        self.settings = self.settings.hide_stdout();
        self
    }

    pub fn hide_stderr(mut self) -> Self {
        self.settings = self.settings.hide_stderr();
        self
    }

    pub fn hide_output(mut self) -> Self {
        self.settings = self.settings.hide_output();
        self
    }

    pub fn run(self) -> Result<(i32, String), Error> {
        exec::exec(self.program, self.args, &self.settings)
    }

    pub fn run_with(self, settings: &Settings) -> Result<(i32, String), Error> {
        exec::exec(self.program, self.args, settings)
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

#[derive(Default)]
pub struct Settings<'a> {
    working_directory: Option<Cow<'a, str>>,
    sudo: Option<Option<Cow<'a, str>>>,
    ssh_client: Option<&'a ssh::SshClient>,
    pipe_input: Vec<Cow<'a, str>>,
    stdout: Option<&'a OsStr>,
    secrets: Vec<&'a str>,
    success_codes: Option<Vec<i32>>,
    silent: bool,
    hide_stdout: bool,
    hide_stderr: bool,
}

impl<'set> Settings<'set> {
    pub fn from_settings(set: &Self) -> Self {
        Self {
            working_directory: set.working_directory.clone(),
            sudo: set.sudo.clone(),
            pipe_input: set.pipe_input.clone(),
            secrets: set.secrets.clone(),
            success_codes: set.success_codes.clone(),
            ..*set
        }
    }

    pub fn ssh(mut self, client: &'set ssh::SshClient) -> Self {
        self.ssh_client = Some(client);
        self
    }

    pub fn working_directory<S: Into<Cow<'set, str>>>(mut self, working_directory: S) -> Self {
        self.working_directory = Some(working_directory.into());
        self
    }

    pub fn sudo(mut self) -> Self {
        self.sudo = Some(None);
        self
    }

    pub fn sudo_password<S: Into<Cow<'set, str>>>(mut self, password: S) -> Self {
        self.sudo = Some(Some(password.into()));
        self
    }

    pub fn stdin_line<S: Into<Cow<'set, str>>>(mut self, input: S) -> Self {
        self.pipe_input.push(input.into());
        self
    }

    pub fn stdin_lines<I, S>(mut self, input: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<Cow<'set, str>>,
    {
        self.pipe_input.extend(input.into_iter().map(Into::into));
        self
    }

    pub fn secret<S: AsRef<str>>(mut self, secret: &'set S) -> Self {
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

    pub fn stdout<S: AsRef<OsStr>>(mut self, path: &'set S) -> Self {
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
