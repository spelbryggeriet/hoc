use std::{
    borrow::Cow,
    future::{Future, IntoFuture},
    pin::Pin,
    process::Stdio,
};

use async_trait::async_trait;
use crossterm::style::Stylize;
use once_cell::sync::OnceCell;
use thiserror::Error;
use tokio::{
    io::{self, AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader},
    join,
    process::Command,
    sync::{Mutex, MutexGuard},
};

use crate::{
    context::kv::{self, Item, Value},
    ledger::{Ledger, Transaction},
    prelude::*,
    prompt,
    util::Opt,
};

pub async fn global_settings<'a>() -> MutexGuard<'a, Settings> {
    static SETTINGS: OnceCell<Mutex<Settings>> = OnceCell::new();

    SETTINGS
        .get_or_init(|| Mutex::new(Settings::new()))
        .lock()
        .await
}

#[derive(Clone)]
pub struct ProcessBuilder<C> {
    handler: C,
    settings: Settings,
}

#[async_trait]
trait ProcessHandler: Send + Sync {
    fn as_raw(&self) -> String;

    async fn on_finished(
        &self,
        _is_forward_process_sudo: bool,
        _revert_process_settings: Settings,
    ) {
    }

    async fn on_revert(
        &self,
        _is_forward_process_sudo: bool,
        _revert_process_settings: Settings,
    ) -> Result<(), Error> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct RegularProcessHandler {
    raw_forward_process: Cow<'static, str>,
}

impl ProcessHandler for RegularProcessHandler {
    fn as_raw(&self) -> String {
        self.raw_forward_process.clone().into_owned()
    }
}

#[derive(Clone)]
pub struct RevertibleProcessHandler {
    raw_forward_process: Cow<'static, str>,
    revert_process: ProcessBuilder<RegularProcessHandler>,
}

impl RevertibleProcessHandler {
    fn get_transaction(
        &self,
        is_forward_process_sudo: bool,
        revert_process_settings: Settings,
    ) -> impl Transaction {
        struct RevertibleTransaction {
            raw_forward_process: Cow<'static, str>,
            is_forward_process_sudo: bool,
            revert_process: ProcessBuilder<RegularProcessHandler>,
            revert_process_settings: Settings,
        }

        #[async_trait]
        impl Transaction for RevertibleTransaction {
            fn description(&self) -> Cow<'static, str> {
                "Run command".into()
            }

            fn detail(&self) -> Cow<'static, str> {
                let sudo_str = util::sudo_string(
                    self.is_forward_process_sudo || self.revert_process_settings.is_sudo(),
                );

                format!(
                    "Command to revert: {}{}\nCommand used to revert: {}{}",
                    self.is_forward_process_sudo
                        .then_some(&*sudo_str)
                        .unwrap_or_default(),
                    self.raw_forward_process.yellow(),
                    self.revert_process_settings
                        .is_sudo()
                        .then_some(&*sudo_str)
                        .unwrap_or_default(),
                    self.revert_process.handler.raw_forward_process.yellow(),
                )
                .into()
            }

            async fn revert(self: Box<Self>) -> anyhow::Result<()> {
                util::run_loop(
                    Box::new(self.revert_process.handler),
                    &self.revert_process_settings,
                    None,
                )
                .await?;
                Ok(())
            }
        }

        RevertibleTransaction {
            raw_forward_process: self.raw_forward_process.clone(),
            is_forward_process_sudo,
            revert_process: self.revert_process.clone(),
            revert_process_settings,
        }
    }
}

#[async_trait]
impl ProcessHandler for RevertibleProcessHandler {
    fn as_raw(&self) -> String {
        self.raw_forward_process.clone().into_owned()
    }

    async fn on_finished(&self, is_forward_process_sudo: bool, revert_process_settings: Settings) {
        let transaction = self.get_transaction(is_forward_process_sudo, revert_process_settings);
        Ledger::get_or_init().lock().await.add(transaction);
    }

    async fn on_revert(
        &self,
        is_forward_process_sudo: bool,
        revert_process_settings: Settings,
    ) -> Result<(), Error> {
        let transaction = self.get_transaction(is_forward_process_sudo, revert_process_settings);

        info!("{}", transaction.detail());

        let opt = select!("Do you want to revert the failed command?")
            .with_options([Opt::Yes, Opt::No])
            .get()?;
        if opt == Opt::Yes {
            Box::new(transaction).revert().await?;
        }
        Ok(())
    }
}

impl ProcessBuilder<RegularProcessHandler> {
    pub fn new(process: Cow<'static, str>) -> Self {
        Self {
            handler: RegularProcessHandler {
                raw_forward_process: process,
            },
            settings: Settings::new(),
        }
    }

    pub fn _revertible(self, revert_process: Self) -> ProcessBuilder<RevertibleProcessHandler> {
        ProcessBuilder {
            handler: RevertibleProcessHandler {
                raw_forward_process: self.handler.raw_forward_process,
                revert_process,
            },
            settings: self.settings,
        }
    }

    #[throws(Error)]
    pub async fn run(self) -> Output {
        let settings: Settings = self.get_current_settings().await;
        util::run_loop(Box::new(self.handler), &settings, None).await?
    }
}

impl ProcessBuilder<RevertibleProcessHandler> {
    #[throws(Error)]
    pub async fn run(self) -> Output {
        let forward_process_settings = self.get_current_settings().await;
        let revert_process_settings = self.handler.revert_process.get_current_settings().await;
        util::run_loop(
            Box::new(self.handler),
            &forward_process_settings,
            Some(revert_process_settings),
        )
        .await?
    }
}

impl<C> ProcessBuilder<C> {
    async fn get_current_settings(&self) -> Settings {
        let mut derived_settings = Settings::new();
        derived_settings.apply(&*global_settings().await);
        derived_settings.apply(&self.settings);
        derived_settings
    }

    pub fn sudo(mut self) -> Self {
        self.settings.sudo();
        self
    }

    pub fn current_dir<P: Into<Cow<'static, str>>>(mut self, current_dir: P) -> Self {
        self.settings.current_dir(current_dir);
        self
    }

    pub fn remote_mode(mut self) -> Self {
        self.settings.remote_mode();
        self
    }
}

type ProcessBuilderFuture = Pin<Box<dyn Future<Output = Result<Output, Error>> + Send>>;

impl IntoFuture for ProcessBuilder<RegularProcessHandler> {
    type IntoFuture = ProcessBuilderFuture;
    type Output = <ProcessBuilderFuture as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

impl IntoFuture for ProcessBuilder<RevertibleProcessHandler> {
    type IntoFuture = ProcessBuilderFuture;
    type Output = <ProcessBuilderFuture as Future>::Output;

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

#[derive(Clone)]
pub struct Settings {
    sudo: Option<bool>,
    current_dir: Option<Option<Cow<'static, str>>>,
    mode: Option<ProcessMode>,
}

impl Settings {
    const DEFAULT_SUDO: bool = false;
    const DEFAULT_CURRENT_DIR: Option<Cow<'static, str>> = None;
    const DEFAULT_MODE: ProcessMode = ProcessMode::Local;

    fn new() -> Self {
        Self {
            sudo: None,
            current_dir: None,
            mode: None,
        }
    }

    fn apply(&mut self, other: &Self) {
        if let Some(sudo) = other.sudo {
            self.sudo.replace(sudo);
        }

        if let Some(current_dir) = &other.current_dir {
            self.current_dir.replace(current_dir.clone());
        }

        if let Some(mode) = other.mode {
            self.mode.replace(mode);
        }
    }

    pub fn sudo(&mut self) -> &mut Self {
        self.sudo.replace(true);
        self
    }

    fn is_sudo(&self) -> bool {
        self.sudo.unwrap_or(Self::DEFAULT_SUDO)
    }

    pub fn current_dir<P: Into<Cow<'static, str>>>(&mut self, current_dir: P) -> &mut Self {
        self.current_dir.replace(Some(current_dir.into()));
        self
    }

    fn get_current_dir(&self) -> Option<Cow<str>> {
        self.current_dir
            .as_ref()
            .map(|opt| opt.as_ref().map(|v| Cow::Borrowed(&**v)))
            .unwrap_or(Self::DEFAULT_CURRENT_DIR)
    }

    pub fn remote_mode(&mut self) -> &mut Self {
        self.mode.replace(ProcessMode::Remote);
        self
    }

    fn get_mode(&self) -> ProcessMode {
        self.mode.unwrap_or(Self::DEFAULT_MODE)
    }
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum ProcessMode {
    #[default]
    Local,
    Remote,
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

mod util {
    use super::*;

    #[throws(Error)]
    pub(super) async fn run_loop(
        mut process_handler: Box<dyn ProcessHandler>,
        forward_process_settings: &Settings,
        mut revert_process_settings: Option<Settings>,
    ) -> Output {
        let maybe_sudo = if forward_process_settings.is_sudo() {
            "sudo -kSp '' "
        } else {
            ""
        };

        let mut raw_process = process_handler.as_raw();
        let mut runnable_process = format!("{maybe_sudo}{raw_process}");
        let mut pipe_input = Vec::new();
        let mut progress_desc = "Running command";

        return loop {
            let mut password_to_cache = None;
            if forward_process_settings.is_sudo() {
                if let Ok(Item::Value(Value::String(password))) = kv!("admin/password").await {
                    pipe_input.push(Cow::Owned(password));
                } else {
                    let password: Secret<String> = prompt!(
                        "[sudo] Password for command {}",
                        raw_process
                            .split_once(' ')
                            .map(|opt| opt.0)
                            .unwrap_or(&raw_process)
                            .yellow()
                    )
                    .without_verification()
                    .hidden()
                    .get()?;
                    password_to_cache.replace(password.clone());
                    pipe_input.push(Cow::Owned(password.into_non_secret()));
                }
            }

            let sudo_str = sudo_string(forward_process_settings.is_sudo());
            let process_str = raw_process.as_str().yellow();
            debug!("{progress_desc}: {sudo_str}{process_str}");
            debug!("Host: this computer");

            if forward_process_settings.get_mode() == ProcessMode::Remote {
                info!("Remote");
            }

            match exec(&runnable_process, &mut pipe_input, forward_process_settings).await {
                Ok(output) => {
                    if let Some(password) = password_to_cache {
                        kv!("admin/password").temporary().put(password).await?;
                    }

                    if let Some(revert_process_settings) = revert_process_settings {
                        process_handler
                            .on_finished(
                                forward_process_settings.is_sudo(),
                                revert_process_settings,
                            )
                            .await;
                    }

                    break output;
                }
                Err(Error::Failed(output)) => {
                    let program = raw_process
                        .split_once(' ')
                        .map(|opt| opt.0)
                        .unwrap_or(&raw_process);

                    error!(
                        "The command {program} failed with exit code {}",
                        output.code,
                    );
                    if !output.stdout.is_empty() {
                        info!("[stdout]\n{}", output.stdout);
                    }
                    if !output.stderr.is_empty() {
                        info!("[stderr]\n{}", output.stderr);
                    }

                    let revert_modify = Opt::Custom("Revert and modify");
                    let revert_rerun = Opt::Custom("Revert and rerun");
                    let mut select = select!("How do you want to resolve the command error?");

                    if revert_process_settings.is_some() {
                        select = select.with_option(revert_modify).with_option(revert_rerun);
                    }

                    let mut opt = match select
                        .with_option(Opt::Modify)
                        .with_option(Opt::Rerun)
                        .with_option(Opt::Skip)
                        .get()
                    {
                        Ok(opt) => opt,
                        Err(err) => {
                            if let Some(revert_process_settings) = revert_process_settings {
                                process_handler
                                    .on_finished(
                                        forward_process_settings.is_sudo(),
                                        revert_process_settings,
                                    )
                                    .await;
                            }

                            throw!(err);
                        }
                    };

                    if [revert_modify, revert_rerun].contains(&opt) {
                        if let Some(revert_process_settings) = revert_process_settings.clone() {
                            process_handler
                                .on_revert(
                                    forward_process_settings.is_sudo(),
                                    revert_process_settings,
                                )
                                .await?;
                        }
                    }

                    if [Opt::Modify, revert_modify].contains(&opt) {
                        let mut prompt = prompt!("New command").with_initial_input(&raw_process);

                        if revert_process_settings.is_some() {
                            prompt = prompt.with_help_message(
                                "Modifying the command will make it non-revertible",
                            );
                        }

                        let new_raw_process = prompt.get()?;
                        if new_raw_process != raw_process {
                            raw_process = new_raw_process;
                            runnable_process = format!("{maybe_sudo}{raw_process}");
                            revert_process_settings.take();
                            process_handler = Box::new(RegularProcessHandler {
                                raw_forward_process: raw_process.clone().into(),
                            });
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
    pub async fn exec(
        raw_process: &str,
        pipe_input: &mut Vec<Cow<'_, str>>,
        settings: &Settings,
    ) -> Output {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", raw_process])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(current_dir) = &settings.get_current_dir() {
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

    pub fn sudo_string(is_sudo: bool) -> Cow<'static, str> {
        if is_sudo {
            format!("{} ", "sudo".black().on_yellow()).into()
        } else {
            "".into()
        }
    }

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
}
