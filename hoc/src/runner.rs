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

use crate::{prelude::*, prompt};

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
    pub async fn run(mut self) -> Output {
        let mut run_progress = Some(progress!("Running command: {}", self.cmd));

        return loop {
            match self.run_impl().await {
                Ok(output) => break output,
                Err(err) => {
                    error!("{err}");
                    run_progress.take();

                    let modify_command = select!("How do you want to resolve the command error?")
                        .with_option("Rerun", || false)
                        .with_option("Modify Command", || true)
                        .get()?;

                    let new_progress = if modify_command {
                        self.cmd =
                            Cow::Owned(prompt!("New command").with_initial_input(&self.cmd).get()?);
                        progress!("Running modified command: {}", self.cmd)
                    } else {
                        progress!("Re-running command: {}", self.cmd)
                    };

                    run_progress.replace(new_progress);
                }
            }
        };
    }

    #[throws(Error)]
    pub async fn run_impl(&self) -> Output {
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
                |line| info!("[stdout] {line}")
            ),
            read_lines(
                child.stderr.take().expect("stderr should exist"),
                |line| warn!("[stderr] {line}")
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

        if status.success() {
            output
        } else {
            throw!(Error::Failed(output))
        }
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

#[derive(Debug)]
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
    #[error("The process failed with exit code {}", _0.code)]
    Failed(Output),

    #[error("The process was terminated by a signal")]
    Terminated,

    #[error(transparent)]
    Prompt(#[from] prompt::Error),

    #[error(transparent)]
    Io(#[from] io::Error),
}
