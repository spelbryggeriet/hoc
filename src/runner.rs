use std::{
    borrow::Cow,
    future::{Future, IntoFuture},
    pin::Pin,
    process::Stdio,
};

use async_trait::async_trait;
use crossterm::style::Stylize;
use thiserror::Error;
use tokio::{
    io::{self, AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader},
    join,
    process::Command,
};

use crate::{
    context::kv::{self, Item, Value},
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
            out.push('\n');
        }
        out.push_str(&line);
    }

    out
}

fn sudo_string(is_sudo: bool) -> Cow<'static, str> {
    if is_sudo {
        format!("{} ", "sudo".black().on_yellow()).into()
    } else {
        "".into()
    }
}

#[derive(Clone)]
pub struct CmdBuilder<C> {
    cmd: C,
    sudo: bool,
    current_dir: Option<Cow<'static, str>>,
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
            current_dir: None,
        }
    }

    pub fn revertible(self, revert_cmd: Self) -> CmdBuilder<Revertible> {
        CmdBuilder {
            cmd: Revertible {
                raw_forward_cmd: self.cmd.0,
                revert_cmd,
            },
            sudo: self.sudo,
            current_dir: self.current_dir,
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
            is_forward_cmd_sudo: bool,
            revert_cmd: CmdBuilder<Regular>,
        }

        #[async_trait]
        impl Transaction for RevertibleTransaction {
            fn description(&self) -> Cow<'static, str> {
                "Run command".into()
            }

            fn detail(&self) -> Cow<'static, str> {
                let sudo_str = sudo_string(self.is_forward_cmd_sudo || self.revert_cmd.sudo);

                format!(
                    "Command to revert: {}{}\nCommand used to revert: {}{}",
                    self.is_forward_cmd_sudo
                        .then_some(&*sudo_str)
                        .unwrap_or_default(),
                    self.forward_cmd.yellow(),
                    self.revert_cmd
                        .sudo
                        .then_some(&*sudo_str)
                        .unwrap_or_default(),
                    self.revert_cmd.cmd.0.yellow(),
                )
                .into()
            }

            async fn revert(self: Box<Self>) -> anyhow::Result<()> {
                self.revert_cmd.await?;
                Ok(())
            }
        }

        RevertibleTransaction {
            forward_cmd: self.cmd.raw_forward_cmd.clone(),
            is_forward_cmd_sudo: self.sudo,
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

        info!("{}", transaction.detail());

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
    pub fn sudo(mut self) -> Self {
        self.sudo = true;
        self
    }

    pub fn current_dir<P: Into<Cow<'static, str>>>(mut self, current_dir: P) -> Self {
        self.current_dir.replace(current_dir.into());
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
        let runnable_sudo_str = if self.sudo { "sudo -kSp '' " } else { "" };

        let mut cmd = get_raw_forward_cmd(&self.cmd);
        let mut runnable_cmd = format!("{runnable_sudo_str}{cmd}");

        let mut pipe_input = Vec::new();

        let mut progress_desc = "Running command";
        return loop {
            let mut password_to_cache = None;
            if self.sudo {
                if let Ok(Item::Value(Value::String(password))) = kv!("admin/password").await {
                    pipe_input.push(Cow::Owned(password));
                } else {
                    let password: Secret<String> = prompt!(
                        "[sudo] Password for command {}",
                        cmd.split_once(' ')
                            .map(|opt| opt.0)
                            .unwrap_or(&cmd)
                            .yellow()
                    )
                    .without_verification()
                    .hidden()
                    .get()?;
                    password_to_cache.replace(password.clone());
                    pipe_input.push(Cow::Owned(password.into_non_secret()));
                }
            }

            if log_enabled!(Level::Debug) {
                let sudo_str = sudo_string(self.sudo);
                let cmd_str = cmd.yellow();
                debug!("{progress_desc}: {sudo_str}{cmd_str}");
                debug!("Host: this computer");
            }

            match self.exec(&runnable_cmd, &mut pipe_input).await {
                Ok(output) => {
                    if let Some(on_ok) = on_ok {
                        on_ok(&self).await;
                    }
                    if let Some(password) = password_to_cache {
                        kv!("admin/password").temporary().put(password).await?;
                    }
                    break output;
                }
                Err(Error::Failed(output)) => {
                    let cmd_only = cmd.split_once(' ').map(|opt| opt.0).unwrap_or(&cmd);

                    // TODO: Due to an unknown bug, a space character is needed at the start or end
                    // to render the icon color.
                    error!(
                        "The command {cmd_only} failed with exit code {}\n ",
                        output.code
                    );
                    error!("[stdout]");
                    if !output.stdout.is_empty() {
                        error!("{}", output.stdout);
                    }
                    error!(" \n[stderr]");
                    if !output.stderr.is_empty() {
                        error!("{}", output.stderr);
                    }

                    let modify = Opt::Custom("Modify");
                    let revert_modify = Opt::Custom("Revert and modify");
                    let revert_rerun = Opt::Custom("Revert and rerun");
                    let mut select = select!("How do you want to resolve the command error?");

                    if is_revertible {
                        select = select.with_option(revert_modify).with_option(revert_rerun);
                    }

                    let mut opt = match select
                        .with_option(modify)
                        .with_option(Opt::Rerun)
                        .with_option(Opt::Skip)
                        .get()
                    {
                        Ok(opt) => opt,
                        Err(err) => {
                            if let Some(on_abort) = on_abort {
                                on_abort(&self).await;
                            }
                            throw!(err);
                        }
                    };

                    if let Some(on_revert) = &on_revert {
                        if [revert_modify, revert_rerun].contains(&opt) {
                            on_revert(&self).await?;
                        }
                    }

                    if [modify, revert_modify].contains(&opt) {
                        let mut prompt = prompt!("New command").with_initial_input(&cmd);

                        if is_revertible {
                            prompt = prompt.with_help_message(
                                "Modifying the command will make it non-revertible",
                            );
                        }

                        let new_cmd = Cow::Owned(prompt.get()?);
                        if new_cmd != cmd {
                            cmd = new_cmd;
                            runnable_cmd = format!("{runnable_sudo_str}{cmd}");
                            is_revertible = false;
                            on_ok.take();
                            on_revert.take();
                            on_abort.take();
                        } else {
                            opt = Opt::Rerun;
                        }
                    } else if opt == Opt::Skip {
                        warn!("Skipping to resolve command error");
                        break output;
                    }

                    progress_desc = if opt == Opt::Rerun {
                        "Re-running command"
                    } else {
                        "Running modified command"
                    };
                }
                Err(err) => throw!(err),
            }
        };
    }

    #[throws(Error)]
    pub async fn exec(&self, raw_cmd: &str, pipe_input: &mut Vec<Cow<'_, str>>) -> Output {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", raw_cmd])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(current_dir) = &self.current_dir {
            cmd.current_dir(&**current_dir);
        }

        let mut child = cmd.spawn()?;

        if !pipe_input.is_empty() {
            let mut stdin = child.stdin.take().expect("stdin should not be taken");
            for input in pipe_input.drain(..) {
                stdin.write_all(input.as_bytes()).await?;
                stdin.write_all(b"\n").await?;
            }
        }

        let mut output = Output::new();
        let (stdout, stderr) = join!(
            read_lines(child.stdout.take().expect("stdout should exist"), |line| {
                debug!("[stdout] {line}")
            }),
            read_lines(child.stderr.take().expect("stderr should exist"), |line| {
                debug!("[stderr] {line}")
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

type CmdBuilderFuture = Pin<Box<dyn Future<Output = Result<Output, Error>> + Send>>;

impl IntoFuture for CmdBuilder<Regular> {
    type IntoFuture = CmdBuilderFuture;
    type Output = <CmdBuilderFuture as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

impl IntoFuture for CmdBuilder<Revertible> {
    type IntoFuture = CmdBuilderFuture;
    type Output = <CmdBuilderFuture as Future>::Output;

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
    Kv(#[from] kv::Error),

    #[error(transparent)]
    Io(#[from] io::Error),

    #[error(transparent)]
    Transaction(#[from] anyhow::Error),
}
