use std::{
    io::{self, Write},
    net::TcpStream,
    path::{Path, PathBuf},
};

use colored::Colorize;
use hoc_log::{error, status};
use thiserror::Error;

use crate::process::{self, ProcessOutput};

#[derive(Debug, Error)]
pub enum SshError {
    #[error("tcp: {0}")]
    Tcp(#[from] io::Error),

    #[error("ssh: {0}")]
    Ssh(#[from] ssh2::Error),
}

impl From<SshError> for hoc_log::Error {
    fn from(err: SshError) -> Self {
        error!("{err}").unwrap_err()
    }
}

pub struct SshClient {
    session: ssh2::Session,
    host: String,
    username: String,
    auth: Authentication,
}

impl SshClient {
    pub fn new_key_auth(
        host: impl AsRef<str> + ToString,
        username: impl AsRef<str> + ToString,
        pub_key_path: impl Into<PathBuf>,
        priv_key_path: impl Into<PathBuf>,
        key_passphrase: impl ToString,
    ) -> Result<Self, SshError> {
        let auth = Authentication::Key {
            pub_key: pub_key_path.into(),
            priv_key: priv_key_path.into(),
            passphrase: key_passphrase.to_string(),
        };
        let session = Self::create_session(host.as_ref(), username.as_ref(), &auth)?;

        Ok(Self {
            session,
            host: host.to_string(),
            username: username.to_string(),
            auth,
        })
    }

    pub fn new_password_auth(
        host: impl AsRef<str> + ToString,
        username: impl AsRef<str> + ToString,
        password: impl ToString,
    ) -> Result<Self, SshError> {
        let auth = Authentication::Password(password.to_string());
        let session = Self::create_session(host.as_ref(), username.as_ref(), &auth)?;

        Ok(Self {
            session,
            host: host.to_string(),
            username: username.to_string(),
            auth,
        })
    }

    pub fn update_key_auth(
        &mut self,
        username: impl AsRef<str> + ToString,
        pub_key_path: impl AsRef<Path> + Into<PathBuf>,
        priv_key_path: impl AsRef<Path> + Into<PathBuf>,
        key_passphrase: impl AsRef<str> + ToString,
    ) -> Result<(), SshError> {
        let same_username = username.as_ref() == &self.username;
        let same_key_auth = matches!(
            &self.auth,
            Authentication::Key {
                pub_key,
                priv_key,
                passphrase,
            } if pub_key == pub_key_path.as_ref()
                && priv_key == priv_key_path.as_ref()
                && passphrase == key_passphrase.as_ref(),
        );

        if !same_username || !same_key_auth {
            self.username = username.to_string();
            self.auth = Authentication::Key {
                pub_key: pub_key_path.into(),
                priv_key: priv_key_path.into(),
                passphrase: key_passphrase.to_string(),
            };
            self.session = Self::create_session(&self.host, &self.username, &self.auth)?;
        }

        Ok(())
    }

    pub fn update_password_auth(
        &mut self,
        username: impl AsRef<str> + ToString,
        password: impl AsRef<str> + ToString,
    ) -> Result<(), SshError> {
        let same_username = username.as_ref() == &self.username;
        let same_password_auth = matches!(
            &self.auth,
            Authentication::Password(pswd) if pswd == password.as_ref(),
        );

        if !same_username || !same_password_auth {
            self.username = username.to_string();
            self.auth = Authentication::Password(password.to_string());
            self.session = Self::create_session(&self.host, &self.username, &self.auth)?;
        }

        Ok(())
    }

    fn create_session(
        host: &str,
        username: &str,
        auth: &Authentication,
    ) -> Result<ssh2::Session, SshError> {
        let host_str = host.blue();
        status!("Connecting to host {host_str}").on(|| {
            let port = 22;
            let stream = TcpStream::connect(format!("{}:{}", host, port))?;

            let mut session = ssh2::Session::new()?;
            session.set_tcp_stream(stream);
            session.handshake()?;

            match auth {
                Authentication::Key {
                    pub_key,
                    priv_key,
                    passphrase,
                } => session.userauth_pubkey_file(
                    username,
                    Some(&pub_key),
                    &priv_key,
                    Some(&passphrase),
                )?,
                Authentication::Password(password) => {
                    session.userauth_password(username, &password)?
                }
            }

            Ok(session)
        })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn spawn<S, B>(&self, cmd: &S, pipe_input: &[B]) -> Result<ssh2::Channel, SshError>
    where
        S: AsRef<str>,
        B: AsRef<str>,
    {
        let mut channel = self.session.channel_session()?;

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
        let err_into = Into::<SshError>::into;
        self.close().map_err(err_into)?;
        self.wait_close().map_err(err_into)?;
        Ok(Some(self.exit_status().map_err(err_into)?))
    }
}

#[derive(PartialEq, Eq)]
enum Authentication {
    Key {
        pub_key: PathBuf,
        priv_key: PathBuf,
        passphrase: String,
    },
    Password(String),
}
