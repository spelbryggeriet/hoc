use std::{
    borrow::Cow,
    fmt::Display,
    future::{Future, IntoFuture},
    pin::Pin,
    process::Stdio,
};

use thiserror::Error;
use tokio::{
    io::{self, AsyncBufReadExt, AsyncRead, BufReader},
    join,
    process::Command,
};

use crate::{
    ledger::{Ledger, Transaction},
    prelude::*,
    prompt,
    util::Opt,
};

#[throws(Error)]
async fn read_lines(reader: impl AsyncRead + Unpin, print_line: impl Fn(&str)) -> String {
    let mut lines = BufReader::new(reader).lines();
    let mut out = String::new();

    while let Some(line) = lines.next_line().await? {
        print_line(&line);
        if !out.is_empty() {
            out.push_str("\n");
        }
        out.push_str(&line);
    }

    out
}

pub struct RunBuilder<C> {
    cmd: C,
    hide_stdout: bool,
    hide_stderr: bool,
}

pub struct Raw(Cow<'static, str>);
pub struct Managed<M>(M);

impl RunBuilder<Raw> {
    pub fn new(raw: impl Into<Cow<'static, str>>) -> Self {
        Self {
            cmd: Raw(raw.into()),
            hide_stdout: false,
            hide_stderr: false,
        }
    }

    pub fn revertible<I: IntoManagedCmd>(
        self,
        managed_cmd: I,
    ) -> RunBuilder<Managed<I::ManagedCmd>> {
        RunBuilder {
            cmd: Managed(managed_cmd.into_managed_cmd(self.cmd.0)),
            hide_stdout: self.hide_stdout,
            hide_stderr: self.hide_stderr,
        }
    }
}

impl<C> RunBuilder<C> {
    pub fn hide_output(mut self) -> Self {
        self.hide_stdout = true;
        self.hide_stderr = true;
        self
    }
}

impl RunBuilder<Raw> {
    #[throws(Error)]
    pub async fn run(self) -> Output {
        async fn noop(_cmd: &mut Raw, _output: &Output) {}

        #[throws(Error)]
        async fn try_noop(_cmd: &mut Raw, _output: &Output) {}

        let mut noop = Some(noop);
        let mut try_noop = Some(try_noop);
        noop.take();
        try_noop.take();

        self.run_loop(
            |cmd| cmd.0.to_owned().into_owned(),
            false,
            noop,
            try_noop,
            noop,
        )
        .await?
    }
}

impl<M: ManagedCmd> RunBuilder<Managed<M>> {
    #[throws(Error)]
    async fn run(self) -> Output {
        async fn add_transaction_to_ledger<M: ManagedCmd>(cmd: &mut Managed<M>, output: &Output) {
            let transaction = cmd.0.get_transaction(output);
            Ledger::get_or_init().lock().await.add(transaction);
        }

        #[throws(Error)]
        async fn revert_transaction<M: ManagedCmd>(cmd: &mut Managed<M>, output: &Output) {
            let transaction = cmd.0.get_transaction(output);
            Box::new(transaction).revert().await?;
        }

        self.run_loop(
            |cmd| cmd.0.as_raw().into_owned(),
            true,
            Some(add_transaction_to_ledger),
            Some(revert_transaction),
            Some(add_transaction_to_ledger),
        )
        .await?
    }
}

impl<C> RunBuilder<C> {
    #[throws(Error)]
    async fn run_loop<O, R, A>(
        mut self,
        get_raw_cmd: impl FnOnce(&C) -> String,
        mut is_managed: bool,
        mut on_ok: Option<O>,
        mut on_revert: Option<R>,
        mut on_abort: Option<A>,
    ) -> Output
    where
        O: for<'a> RunLoopEventFn<'a, C, ()>,
        R: for<'a> RunLoopEventFn<'a, C, Result<(), Error>>,
        A: for<'a> RunLoopEventFn<'a, C, ()>,
    {
        let mut cmd = get_raw_cmd(&self.cmd);
        let mut run_progress = Some(progress_with_handle!("Running command: {cmd}"));

        let mut retrying = false;
        return loop {
            info!("Host: this computer");

            match self.exec(&cmd, retrying).await {
                Ok(output) => {
                    if let Some(on_ok) = on_ok {
                        on_ok(&mut self.cmd, &output).await;
                    }
                    break output;
                }
                Err(Error::Failed(output)) => {
                    error!("The process failed with exit code {}", output.code);

                    let modify_command = Opt::Custom("Modify command");
                    let revert_and_rerun = Opt::Custom("Revert and rerun");
                    let mut select = select!("How do you want to resolve the command error?")
                        .with_option(modify_command)
                        .with_option(Opt::Rerun);

                    if is_managed {
                        select = select.with_option(revert_and_rerun);
                    }

                    let opt = match select.get() {
                        Ok(opt) => opt,
                        Err(err) => {
                            if let Some(on_abort) = on_abort {
                                on_abort(&mut self.cmd, &output).await;
                            }
                            throw!(err);
                        }
                    };

                    run_progress.take();

                    let new_progress = if opt == Opt::Rerun {
                        progress_with_handle!("Re-running command: {cmd}")
                    } else if opt == modify_command {
                        cmd = prompt!("New command").with_initial_input(&cmd).get()?;
                        is_managed = false;
                        on_ok.take();
                        on_revert.take();
                        on_abort.take();

                        progress_with_handle!("Running modified command: {cmd}")
                    } else if opt == revert_and_rerun {
                        if let Some(on_revert) = &on_revert {
                            run_progress.replace(progress_with_handle!("Reverting command: {cmd}"));
                            on_revert(&mut self.cmd, &output).await?;
                        }

                        run_progress.take();
                        progress_with_handle!("Re-running command: {cmd}")
                    } else {
                        unreachable!();
                    };

                    run_progress.replace(new_progress);
                    retrying = true;
                }
                Err(err) => throw!(err),
            }
        };
    }

    #[throws(Error)]
    pub async fn exec(&self, raw_cmd: &str, override_show_output: bool) -> Output {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", raw_cmd])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn()?;

        if !override_show_output {
            if self.hide_stdout && self.hide_stderr {
                info!("Output hidden");
            } else if self.hide_stdout {
                info!("Standard output hidden");
            } else if self.hide_stderr {
                info!("Standard error hidden");
            }
        }

        let mut output = Output::new();
        let (stdout, stderr) = join!(
            read_lines(child.stdout.take().expect("stdout should exist"), |line| {
                if override_show_output || !self.hide_stdout {
                    info!("[stdout] {line}")
                }
            }),
            read_lines(child.stderr.take().expect("stderr should exist"), |line| {
                if override_show_output || !self.hide_stderr {
                    warn!("[stderr] {line}")
                }
            }),
        );

        output.stdout = stdout?;
        output.stderr = stderr?;

        let status = child.wait().await?;

        let Some(code) = status.code() else {
            throw!(Error::Terminated)
        };
        output.code = code;

        if status.success() {
            output
        } else {
            throw!(Error::Failed(output))
        }
    }
}

type RunBuilderFuture = Pin<Box<dyn Future<Output = Result<Output, Error>> + Send + 'static>>;

impl IntoFuture for RunBuilder<Raw> {
    type IntoFuture = RunBuilderFuture;
    type Output = <RunBuilderFuture as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

impl<M: ManagedCmd> IntoFuture for RunBuilder<Managed<M>> {
    type IntoFuture = RunBuilderFuture;
    type Output = <RunBuilderFuture as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

pub trait RunLoopEventFn<'a, C: 'a, O>: Fn(&'a mut C, &'a Output) -> Self::Fut {
    type Fut: Future<Output = O>;
}

impl<'a, C: 'a, O, F, Fut> RunLoopEventFn<'a, C, O> for F
where
    F: Fn(&'a mut C, &'a Output) -> Fut,
    Fut: Future<Output = O>,
{
    type Fut = Fut;
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

pub trait ManagedCmd: Display + Send + Sync + 'static {
    type Transaction: Transaction;

    fn get_transaction(&self, output: &Output) -> Self::Transaction;
    fn as_raw(&self) -> Cow<str>;
}

pub trait IntoManagedCmd {
    type ManagedCmd: ManagedCmd;

    fn into_managed_cmd(self, raw: Cow<'static, str>) -> Self::ManagedCmd;
}

impl<M, F> IntoManagedCmd for F
where
    M: ManagedCmd,
    F: FnOnce(Cow<'static, str>) -> M,
{
    type ManagedCmd = M;

    fn into_managed_cmd(self, raw: Cow<'static, str>) -> Self::ManagedCmd {
        self(raw)
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

    #[error(transparent)]
    Transaction(#[from] anyhow::Error),
}
