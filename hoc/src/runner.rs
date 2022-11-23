use std::{
    borrow::Cow,
    future::{Future, IntoFuture},
    pin::Pin,
    process::Stdio,
};

use async_trait::async_trait;
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

#[derive(Clone)]
pub struct CmdBuilder<C> {
    cmd: C,
    sudo: bool,
    hide_stdout: bool,
    hide_stderr: bool,
}

#[derive(Clone)]
pub struct Regular(Cow<'static, str>);

#[derive(Clone)]
pub struct Revertible {
    raw_forward_cmd: Cow<'static, str>,
    revert_cmd: CmdBuilder<Regular>,
}

impl CmdBuilder<Regular> {
    pub fn new(cmd: Cow<'static, str>) -> Self {
        Self {
            cmd: Regular(cmd),
            sudo: false,
            hide_stdout: false,
            hide_stderr: false,
        }
    }

    pub fn _revertible(self, revert_cmd: Self) -> CmdBuilder<Revertible> {
        CmdBuilder {
            cmd: Revertible {
                raw_forward_cmd: self.cmd.0,
                revert_cmd,
            },
            sudo: false,
            hide_stdout: false,
            hide_stderr: false,
        }
    }

    async fn noop(&self) {}

    #[throws(Error)]
    async fn try_noop(&self) {}

    pub async fn run(self) -> Result<Output, Error> {
        let mut noop = Some(Self::noop);
        let mut try_noop = Some(Self::try_noop);
        noop.take();
        try_noop.take();

        self.run_loop(|cmd| Cow::Borrowed(&cmd.0), false, noop, try_noop, noop)
            .await
    }
}

impl CmdBuilder<Revertible> {
    fn get_transaction(&self) -> impl Transaction {
        struct RevertibleTransaction {
            forward_cmd: Cow<'static, str>,
            revert_cmd: CmdBuilder<Regular>,
        }

        #[async_trait]
        impl Transaction for RevertibleTransaction {
            fn description(&self) -> Cow<'static, str> {
                "Run command".into()
            }

            fn detail(&self) -> Cow<'static, str> {
                format!("Command to revert: {}", self.forward_cmd).into()
            }

            async fn revert(self: Box<Self>) -> anyhow::Result<()> {
                self.revert_cmd.await?;
                Ok(())
            }
        }

        RevertibleTransaction {
            forward_cmd: self.cmd.raw_forward_cmd.to_owned(),
            revert_cmd: self.cmd.revert_cmd.clone(),
        }
    }

    async fn add_transaction_to_ledger(&self) {
        let transaction = self.get_transaction();
        Ledger::get_or_init().lock().await.add(transaction);
    }

    #[throws(Error)]
    async fn revert_transaction(&self) {
        let transaction = self.get_transaction();

        let revert_cmd = &self.cmd.revert_cmd.cmd.0;
        info!("Revert command available: {revert_cmd}");
        let opt = select!("Do you want to revert the failed command?")
            .with_options([Opt::Yes, Opt::No])
            .get()?;
        if opt == Opt::Yes {
            Box::new(transaction).revert().await?;
        }
    }

    #[throws(Error)]
    pub async fn run(self) -> Output {
        self.run_loop(
            |cmd| Cow::Borrowed(&cmd.raw_forward_cmd),
            true,
            Some(Self::add_transaction_to_ledger),
            Some(Self::revert_transaction),
            Some(Self::add_transaction_to_ledger),
        )
        .await?
    }
}

impl<C> CmdBuilder<C> {
    pub fn hide_output(mut self) -> Self {
        self.hide_stdout = true;
        self.hide_stderr = true;
        self
    }

    pub fn sudo(mut self) -> Self {
        self.sudo = true;
        self
    }
}

impl CmdBuilder<Revertible> {}

impl<C> CmdBuilder<C> {
    #[throws(Error)]
    async fn run_loop<O, R, A>(
        self,
        get_raw_forward_cmd: impl FnOnce(&C) -> Cow<str>,
        mut is_revertible: bool,
        mut on_ok: Option<O>,
        mut on_revert: Option<R>,
        mut on_abort: Option<A>,
    ) -> Output
    where
        O: for<'a> RunLoopEventFn<'a, Self, ()>,
        R: for<'a> RunLoopEventFn<'a, Self, Result<(), Error>>,
        A: for<'a> RunLoopEventFn<'a, Self, ()>,
    {
        let mut cmd = get_raw_forward_cmd(&self.cmd);
        if self.sudo {
            cmd = Cow::Owned(format!("sudo {cmd}"));
        }

        let mut run_progress = Some(progress_with_handle!("Running command: {cmd}"));

        let mut retrying = false;
        return loop {
            info!("Host: this computer");

            match self.exec(&cmd, retrying).await {
                Ok(output) => {
                    if let Some(on_ok) = on_ok {
                        on_ok(&self).await;
                    }
                    break output;
                }
                Err(Error::Failed(output)) => {
                    error!("The process failed with exit code {}", output.code);

                    if is_revertible {
                        if let Some(on_revert) = &on_revert {
                            on_revert(&self).await?;
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
                                on_abort(&self).await;
                            }
                            throw!(err);
                        }
                    };

                    if opt == modify_command {
                        let mut prompt = prompt!("New command").with_initial_input(&cmd);

                        if is_revertible {
                            prompt = prompt.with_help_message(
                                "Modifying the command will make it non-revertible",
                            );
                        }

                        let new_cmd = Cow::Owned(prompt.get()?);
                        if new_cmd != cmd {
                            cmd = new_cmd;
                            is_revertible = false;
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

impl IntoFuture for CmdBuilder<Regular> {
    type IntoFuture = RunBuilderFuture;
    type Output = <RunBuilderFuture as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

impl IntoFuture for CmdBuilder<Revertible> {
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
