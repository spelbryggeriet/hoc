use std::{
    io::{self, Write},
    net::TcpStream,
    path::Path,
};

use thiserror::Error;

use crate::{command::util::ProcessOutput, StdResult};

type Result<T> = StdResult<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("tcp error: {0}")]
    Tcp(#[from] io::Error),

    #[error("ssh error: {0}")]
    Ssh(#[from] ssh2::Error),
}

pub struct Client {
    host: String,
    session: ssh2::Session,
}

impl Client {
    pub fn new(host: String, username: &str, auth: Authentication) -> Result<Self> {
        let session = Self::create_session(&host, username, auth)?;

        Ok(Self { host, session })
    }

    fn create_session(host: &str, username: &str, auth: Authentication) -> Result<ssh2::Session> {
        let port = 22;
        let stream = TcpStream::connect(format!("{}:{}", host, port))?;

        let mut session = ssh2::Session::new()?;
        session.set_tcp_stream(stream);
        session.handshake()?;

        match auth {
            Authentication::KeyBased { pub_key, priv_key } => session.userauth_pubkey_file(
                username,
                Some(pub_key.as_ref()),
                priv_key.as_ref(),
                None,
            )?,
            Authentication::Password(password) => {
                session.userauth_password(username, password.as_ref())?
            }
        }

        Ok(session)
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn spawn<S, B>(&self, cmd: &S, pipe_input: Option<&[B]>) -> Result<ssh2::Channel>
    where
        S: AsRef<str>,
        B: AsRef<[u8]>,
    {
        let mut channel = self.session.channel_session()?;

        channel.exec(cmd.as_ref())?;

        if let Some(pipe_input) = pipe_input {
            for input in pipe_input {
                channel.write_all(input.as_ref())?;
                channel.write_all(b"\n")?;
            }
        };

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

    fn finish(mut self) -> super::Result<Option<i32>> {
        let err_into = Into::<Error>::into;
        self.close().map_err(err_into)?;
        self.wait_close().map_err(err_into)?;
        Ok(Some(self.exit_status().map_err(err_into)?))
    }
}

pub enum Authentication<'a> {
    Password(&'a str),
    KeyBased {
        pub_key: &'a Path,
        priv_key: &'a Path,
    },
}
