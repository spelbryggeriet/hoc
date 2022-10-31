use std::{
    borrow::Cow,
    future::{Future, IntoFuture},
    pin::Pin,
};

use async_process::{Command, Stdio};
use async_std::{
    io::{self, prelude::BufReadExt, BufReader},
    stream::StreamExt,
};
use futures::{join, AsyncRead};
use thiserror::Error;

use crate::prelude::*;

#[throws(Error)]
async fn read_lines(reader: impl AsyncRead + Unpin, print_line: impl Fn(&str)) -> String {
    let mut lines = BufReader::new(reader).lines();
    let mut out = String::new();

    while let Some(line) = lines.next().await {
        let line = line?;

        print_line(&line);
        if !out.is_empty() {
            out.push_str("\n");
        }
        out.push_str(&line);
    }

    out
}

pub struct RunBuilder {
    cmd: Cow<'static, str>,
}

impl RunBuilder {
    pub fn new(cmd: impl Into<Cow<'static, str>>) -> Self {
        Self { cmd: cmd.into() }
    }

    #[throws(Error)]
    pub async fn run(self) -> Output {
        progress_scoped!("Running command: {}", self.cmd);

        self.run_impl().await?
    }

    #[throws(Error)]
    pub async fn run_impl(self) -> Output {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &self.cmd])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn()?;

        let mut output = Output::new();
        let (stdout, stderr) = join!(
            read_lines(
                child.stdout.take().expect("stdout should exist"),
                |line| info!("{line}")
            ),
            read_lines(
                child.stderr.take().expect("stderr should exist"),
                |line| warn!("{line}")
            ),
        );

        output.stdout = stdout?;
        output.stderr = stderr?;

        let status = child.status().await?;

        output.code = if let Some(code) = status.code() {
            code
        } else {
            throw!(Error::Terminated)
        };

        output
    }
}

type RunBuilderFuture = Pin<Box<dyn Future<Output = Result<Output, Error>>>>;
impl IntoFuture for RunBuilder {
    type IntoFuture = RunBuilderFuture;
    type Output = <RunBuilderFuture as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    fn new() -> Self {
        Self {
            code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("The process was terminated by a signal")]
    Terminated,

    #[error(transparent)]
    Io(#[from] io::Error),
}
