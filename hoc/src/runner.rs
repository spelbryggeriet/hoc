use std::{
    borrow::Cow,
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
pub struct Transactional<T>(T);

impl RunBuilder<Raw> {
    pub fn raw(raw: Cow<'static, str>) -> Self {
        Self {
            cmd: Raw(raw),
            hide_stdout: false,
            hide_stderr: false,
        }
    }
}

impl<T: TransactionalCmd> RunBuilder<Transactional<T>> {
    pub fn _transactional(transactional: T) -> Self {
        Self {
            cmd: Transactional(transactional),
            hide_stdout: false,
            hide_stderr: false,
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
        async fn noop(_cmd: &Raw) {}

        #[throws(Error)]
        async fn try_noop(_cmd: &Raw) {}

        let mut noop = Some(noop);
        let mut try_noop = Some(try_noop);
        noop.take();
        try_noop.take();

        self.run_loop(|cmd| Cow::Borrowed(&cmd.0), false, noop, try_noop, noop)
            .await?
    }
}

impl<T: TransactionalCmd> RunBuilder<Transactional<T>> {
    #[throws(Error)]
    pub async fn run(self) -> Output {
        async fn add_transaction_to_ledger<T: TransactionalCmd>(cmd: &Transactional<T>) {
            let transaction = cmd.0.get_transaction();
            Ledger::get_or_init().lock().await.add(transaction);
        }

        #[throws(Error)]
        async fn revert_transaction<T: TransactionalCmd>(cmd: &Transactional<T>) {
            let transaction = cmd.0.get_transaction();

            let revert_cmd = cmd.0.revert_cmd();
            info!("Revert command available: {revert_cmd}");
            let opt = select!("Do you want to revert the failed command?")
                .with_options([Opt::Yes, Opt::No])
                .get()?;
            if opt == Opt::Yes {
                Box::new(transaction).revert().await?;
            }
        }

        self.run_loop(
            |cmd| cmd.0.forward_cmd(),
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
        self,
        get_raw_cmd: impl FnOnce(&C) -> Cow<str>,
        mut is_transactional: bool,
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
                        on_ok(&self.cmd).await;
                    }
                    break output;
                }
                Err(Error::Failed(output)) => {
                    error!("The process failed with exit code {}", output.code);

                    if is_transactional {
                        if let Some(on_revert) = &on_revert {
                            on_revert(&self.cmd).await?;
                        }
                    }

                    let modify_command = Opt::Custom("Modify command");
                    let select = select!("How do you want to resolve the command error?")
                        .with_option(modify_command)
                        .with_option(Opt::Rerun);
                    let mut opt = match select.get() {
                        Ok(opt) => opt,
                        Err(err) => {
                            if let Some(on_abort) = on_abort {
                                on_abort(&self.cmd).await;
                            }
                            throw!(err);
                        }
                    };

                    if opt == modify_command {
                        let mut prompt = prompt!("New command").with_initial_input(&cmd);

                        if is_transactional {
                            prompt = prompt.with_help_message(
                                "Modifying the command will make it non-revertible",
                            );
                        }

                        let new_cmd = Cow::Owned(prompt.get()?);
                        if new_cmd != cmd {
                            cmd = new_cmd;
                            is_transactional = false;
                            on_ok.take();
                            on_revert.take();
                            on_abort.take();
                        } else {
                            opt = Opt::Rerun;
                        }
                    }

                    run_progress.take();
                    let new_progress = if opt == Opt::Rerun {
                        progress_with_handle!("Re-running command: {cmd}")
                    } else {
                        progress_with_handle!("Running modified command: {cmd}")
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

type RunBuilderFuture = Pin<Box<dyn Future<Output = Result<Output, Error>> + Send>>;

impl IntoFuture for RunBuilder<Raw> {
    type IntoFuture = RunBuilderFuture;
    type Output = <RunBuilderFuture as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

impl<T: TransactionalCmd> IntoFuture for RunBuilder<Transactional<T>> {
    type IntoFuture = RunBuilderFuture;
    type Output = <RunBuilderFuture as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

pub trait RunLoopEventFn<'a, C: 'a, O>: Fn(&'a C) -> Self::Fut {
    type Fut: Future<Output = O>;
}

impl<'a, C: 'a, O, F, Fut> RunLoopEventFn<'a, C, O> for F
where
    F: Fn(&'a C) -> Fut,
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

pub trait TransactionalCmd: Send + Sync + 'static {
    type Transaction: Transaction;

    fn get_transaction(&self) -> Self::Transaction;
    fn forward_cmd(&self) -> Cow<str>;
    fn revert_cmd(&self) -> Cow<str>;
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
