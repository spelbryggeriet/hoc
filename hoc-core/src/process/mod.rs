use std::{
    borrow::Cow,
    collections::HashMap,
    ffi::OsStr,
    io::{self, BufRead, BufReader, Read},
    process,
};

use colored::Colorize;
use hoc_log::{error, info};
use thiserror::Error;

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
    fn obfuscate<S: AsRef<str>>(self, secrets: &[S]) -> Cow<'a, str>;
}

impl<'a> Obfuscate<'a> for Cow<'a, str> {
    fn obfuscate<S: AsRef<str>>(mut self, secrets: &[S]) -> Cow<'a, str> {
        for secret in secrets.into_iter() {
            if self.contains(secret.as_ref()) {
                self = Cow::Owned(self.replace(secret.as_ref(), &"<obfuscated>".red().to_string()));
            }
        }
        self
    }
}

impl<'a> Obfuscate<'a> for String {
    fn obfuscate<S: AsRef<str>>(self, secrets: &[S]) -> Cow<'a, str> {
        Cow::<str>::Owned(self).obfuscate(secrets)
    }
}

impl<'a> Obfuscate<'a> for &'a str {
    fn obfuscate<S: AsRef<str>>(self, secrets: &[S]) -> Cow<'a, str> {
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

impl<'a> Quotify<'a> for &'a Cow<'a, str> {
    fn needs_quotes(&self) -> bool {
        Cow::needs_quotes(self)
    }

    fn quotify(self) -> Cow<'a, str> {
        Cow::quotify(Cow::Borrowed(self))
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
    Ssh(#[from] ssh::Error),

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

impl<'proc> Process<'proc> {
    pub fn cmd<S: AsRef<OsStr>>(program: &'proc S) -> Self {
        Self {
            program: program.as_ref(),
            args: Vec::new(),
            settings: Settings::default(),
        }
    }

    pub fn arg<S: AsRef<OsStr>>(mut self, arg: &'proc S) -> Self {
        self.args.push(Cow::Borrowed(arg.as_ref()));
        self
    }

    pub fn settings(mut self, settings: &'proc Settings) -> Self {
        self.settings = settings.clone();
        self
    }

    pub fn env<K, V>(mut self, key: K, value: V) -> Self
    where
        K: Into<Cow<'proc, str>>,
        V: Into<Cow<'proc, str>>,
    {
        self.settings = self.settings.env(key, value);
        self
    }

    pub fn ssh(mut self, client: &'proc ssh::Client) -> Self {
        self.settings = self.settings.ssh(client);
        self
    }

    pub fn sudo(mut self) -> Self {
        self.settings = self.settings.sudo();
        self
    }

    pub fn sudo_password<S: Into<Cow<'proc, str>>>(mut self, password: S) -> Self {
        self.settings = self.settings.sudo_password(password);
        self
    }

    pub fn sudo_user<S: AsRef<OsStr>>(mut self, user: &'proc S) -> Self {
        self.settings = self.settings.sudo_user(user);
        self
    }

    pub fn stdin_line<S: Into<Cow<'proc, str>>>(mut self, input: S) -> Self {
        self.settings = self.settings.stdin_line(input);
        self
    }

    pub fn stdin_lines<I, S>(mut self, input: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<Cow<'proc, str>>,
    {
        self.settings = self.settings.stdin_lines(input);
        self
    }

    pub fn secret<S: Into<Cow<'proc, str>>>(mut self, secret: S) -> Self {
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

    pub fn stdout<S: AsRef<OsStr>>(mut self, path: &'proc S) -> Self {
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

    fn read_stdout_to_string<S: AsRef<str>>(
        &mut self,
        show_output: bool,
        secrets: &[S],
    ) -> Result<String, Error> {
        Self::read_lines(self.stdout(), show_output, secrets)
    }

    fn read_stderr_to_string<S: AsRef<str>>(
        &mut self,
        show_output: bool,
        secrets: &[S],
    ) -> Result<String, Error> {
        Self::read_lines(self.stderr(), show_output, secrets)
    }

    fn read_lines<S: AsRef<str>>(
        reader: impl Read,
        show_output: bool,
        secrets: &[S],
    ) -> Result<String, Error> {
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

#[derive(Default, Clone)]
pub struct Settings<'a> {
    env: HashMap<Cow<'a, str>, Cow<'a, str>>,
    hide_stderr: bool,
    hide_stdout: bool,
    pipe_input: Vec<Cow<'a, str>>,
    secrets: Vec<Cow<'a, str>>,
    silent: bool,
    ssh_client: Option<&'a ssh::Client>,
    stdout: Option<&'a OsStr>,
    success_codes: Option<Vec<i32>>,
    sudo: Option<(Option<Cow<'a, str>>, Option<&'a OsStr>)>,
}

impl<'set> Settings<'set> {
    pub fn env<K, V>(mut self, key: K, value: V) -> Self
    where
        K: Into<Cow<'set, str>>,
        V: Into<Cow<'set, str>>,
    {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn ssh(mut self, client: &'set ssh::Client) -> Self {
        self.ssh_client = Some(client);
        self
    }

    pub fn sudo(mut self) -> Self {
        self.sudo = Some((None, None));
        self
    }

    pub fn sudo_password<S: Into<Cow<'set, str>>>(mut self, password: S) -> Self {
        let password = Some(password.into());
        let user = self.sudo.map_or(None, |(_, user)| user);
        self.sudo = Some((password, user));
        self
    }

    pub fn sudo_user<S: AsRef<OsStr>>(mut self, user: &'set S) -> Self {
        let password = self.sudo.map_or(None, |(password, _)| password);
        let user = Some(user.as_ref());
        self.sudo = Some((password, user));
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

    pub fn secret<S: Into<Cow<'set, str>>>(mut self, secret: S) -> Self {
        self.secrets.push(secret.into());
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
        self.hide_output()
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
