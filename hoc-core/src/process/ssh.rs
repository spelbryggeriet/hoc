use std::{
    cell::{Ref, RefCell},
    io::{self, Write},
    net::TcpStream,
    path::PathBuf,
};

use colored::Colorize;
use hoc_log::{error, status};
use thiserror::Error;

use crate::process::{self, ProcessOutput};

#[derive(Debug, Error)]
pub enum Error {
    #[error("host not configured")]
    Host,

    #[error("user not configured")]
    User,

    #[error("password not configured")]
    Password,

    #[error("authentication not configured")]
    Auth,

    #[error("tcp: {0}")]
    Tcp(#[from] io::Error),

    #[error(transparent)]
    Ssh(#[from] ssh2::Error),
}

impl From<Error> for hoc_log::Error {
    fn from(err: Error) -> Self {
        error!("{err}").unwrap_err()
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub host: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub auth: Option<Authentication>,
}

impl Options {
    pub fn host(mut self, host: impl ToString) -> Self {
        self.host.replace(host.to_string());
        self
    }

    pub fn user(mut self, user: impl ToString) -> Self {
        self.user.replace(user.to_string());
        self
    }

    pub fn password(mut self, password: impl ToString) -> Self {
        self.password.replace(password.to_string());
        self
    }

    pub fn password_auth(mut self) -> Self {
        self.auth.replace(Authentication::Password);
        self
    }

    pub fn key_auth(
        mut self,
        pub_key_path: impl Into<PathBuf>,
        priv_key_path: impl Into<PathBuf>,
    ) -> Self {
        self.auth.replace(Authentication::Key {
            pub_key_path: pub_key_path.into(),
            priv_key_path: priv_key_path.into(),
        });
        self
    }
}

#[derive(Default)]
pub struct Client {
    session: RefCell<Option<ssh2::Session>>,
    options: RefCell<Options>,
}

impl Client {
    pub(super) fn options(&self) -> Ref<Options> {
        self.options.borrow()
    }

    pub fn update(&self, options: Options) {
        let mut ref_mut = self.options.borrow_mut();
        if *ref_mut != options {
            *ref_mut = options;
            self.disconnect();
        }
    }

    pub fn connect(&self) -> Result<(), Error> {
        let mut ref_mut = self.session.borrow_mut();
        if ref_mut.is_none() {
            ref_mut.replace(self.create_session()?);
        }
        Ok(())
    }

    pub fn disconnect(&self) {
        self.session.borrow_mut().take();
    }

    fn create_session(&self) -> Result<ssh2::Session, Error> {
        let options = self.options();
        let host = options.host.as_ref().ok_or(Error::Host)?;
        let user = options.user.as_ref().ok_or(Error::User)?;
        let password = options.password.as_ref().ok_or(Error::Password)?;
        let auth = options.auth.as_ref().ok_or(Error::Auth)?;

        let host_str = host.blue();
        status!("Connecting to host {host_str}").on(|| {
            let port = 22;
            let stream = TcpStream::connect(format!("{}:{}", host, port))?;

            let mut session = ssh2::Session::new()?;
            session.set_tcp_stream(stream);
            session.handshake()?;

            match auth {
                Authentication::Key {
                    pub_key_path,
                    priv_key_path,
                } => session.userauth_pubkey_file(
                    user,
                    Some(&pub_key_path),
                    &priv_key_path,
                    Some(&password),
                )?,
                Authentication::Password => session.userauth_password(user, &password)?,
            }

            Ok(session)
        })
    }

    pub fn spawn<S, B>(&self, cmd: &S, pipe_input: &[B]) -> Result<ssh2::Channel, Error>
    where
        S: AsRef<str>,
        B: AsRef<str>,
    {
        self.connect()?;
        let mut channel = self.session.borrow().as_ref().unwrap().channel_session()?;

        channel.exec(cmd.as_ref())?;

        for input in pipe_input {
            channel.write_all(input.as_ref().as_bytes())?;
            channel.write_all(b"\n")?;
        }

        channel.send_eof()?;

        Ok(channel)
    }
}

impl ProcessOutput for ssh2::Channel {
    type Stdout = ssh2::Stream;
    type Stderr = ssh2::Stream;

    fn stdout(&mut self) -> Self::Stdout {
        self.stream(0)
    }

    fn stderr(&mut self) -> Self::Stderr {
        ssh2::Channel::stderr(self)
    }

    fn finish(mut self) -> Result<Option<i32>, process::Error> {
        let err_into = Into::<Error>::into;
        self.close().map_err(err_into)?;
        self.wait_close().map_err(err_into)?;
        Ok(Some(self.exit_status().map_err(err_into)?))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Authentication<P = PathBuf> {
    Password,
    Key { pub_key_path: P, priv_key_path: P },
}
