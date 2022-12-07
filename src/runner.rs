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
    static SETTINGS_QUEUE: OnceCell<Mutex<Settings>> = OnceCell::new();

    SETTINGS_QUEUE
        .get_or_init(|| Mutex::new(Settings::new()))
        .lock()
        .await
}

#[derive(Clone)]
pub struct CmdBuilder<C> {
    cmd: C,
    settings: Settings,
}

#[async_trait]
trait Cmd: Send + Sync {
    fn as_raw(&self) -> String;

    async fn on_finished(&self, _is_forward_cmd_sudo: bool, _revert_cmd_settings: Settings) {}

    async fn on_revert(
        &self,
        _is_forward_cmd_sudo: bool,
        _revert_cmd_settings: Settings,
    ) -> Result<(), Error> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct RegularCmd {
    raw_forward_cmd: Cow<'static, str>,
}

impl Cmd for RegularCmd {
    fn as_raw(&self) -> String {
        self.raw_forward_cmd.clone().into_owned()
    }
}

#[derive(Clone)]
pub struct RevertibleCmd {
    raw_forward_cmd: Cow<'static, str>,
    revert_cmd: CmdBuilder<RegularCmd>,
}

impl RevertibleCmd {
    fn get_transaction(
        &self,
        is_forward_cmd_sudo: bool,
        revert_cmd_settings: Settings,
    ) -> impl Transaction {
        struct RevertibleTransaction {
            raw_forward_cmd: Cow<'static, str>,
            is_forward_cmd_sudo: bool,
            revert_cmd: CmdBuilder<RegularCmd>,
            revert_cmd_settings: Settings,
        }

        #[async_trait]
        impl Transaction for RevertibleTransaction {
            fn description(&self) -> Cow<'static, str> {
                "Run command".into()
            }

            fn detail(&self) -> Cow<'static, str> {
                let sudo_str = util::sudo_string(
                    self.is_forward_cmd_sudo || self.revert_cmd_settings.is_sudo(),
                );

                format!(
                    "Command to revert: {}{}\nCommand used to revert: {}{}",
                    self.is_forward_cmd_sudo
                        .then_some(&*sudo_str)
                        .unwrap_or_default(),
                    self.raw_forward_cmd.yellow(),
                    self.revert_cmd_settings
                        .is_sudo()
                        .then_some(&*sudo_str)
                        .unwrap_or_default(),
                    self.revert_cmd.cmd.raw_forward_cmd.yellow(),
                )
                .into()
            }

            async fn revert(self: Box<Self>) -> anyhow::Result<()> {
                util::run_loop(
                    Box::new(self.revert_cmd.cmd),
                    &self.revert_cmd_settings,
                    None,
                )
                .await?;
                Ok(())
            }
        }

        RevertibleTransaction {
            raw_forward_cmd: self.raw_forward_cmd.clone(),
            is_forward_cmd_sudo,
            revert_cmd: self.revert_cmd.clone(),
            revert_cmd_settings,
        }
    }
}

#[async_trait]
impl Cmd for RevertibleCmd {
    fn as_raw(&self) -> String {
        self.raw_forward_cmd.clone().into_owned()
    }

    async fn on_finished(&self, is_forward_cmd_sudo: bool, revert_cmd_settings: Settings) {
        let transaction = self.get_transaction(is_forward_cmd_sudo, revert_cmd_settings);
        Ledger::get_or_init().lock().await.add(transaction);
    }

    async fn on_revert(
        &self,
        is_forward_cmd_sudo: bool,
        revert_cmd_settings: Settings,
    ) -> Result<(), Error> {
        let transaction = self.get_transaction(is_forward_cmd_sudo, revert_cmd_settings);

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

impl CmdBuilder<RegularCmd> {
    pub fn new(cmd: Cow<'static, str>) -> Self {
        Self {
            cmd: RegularCmd {
                raw_forward_cmd: cmd,
            },
            settings: Settings::new(),
        }
    }

    pub fn _revertible(self, revert_cmd: Self) -> CmdBuilder<RevertibleCmd> {
        CmdBuilder {
            cmd: RevertibleCmd {
                raw_forward_cmd: self.cmd.raw_forward_cmd,
                revert_cmd,
            },
            settings: self.settings,
        }
    }

    #[throws(Error)]
    pub async fn run(self) -> Output {
        let settings: Settings = self.get_current_settings().await;
        util::run_loop(Box::new(self.cmd), &settings, None).await?
    }
}

impl CmdBuilder<RevertibleCmd> {
    #[throws(Error)]
    pub async fn run(self) -> Output {
        let forward_cmd_settings = self.get_current_settings().await;
        let revert_cmd_settings = self.cmd.revert_cmd.get_current_settings().await;
        util::run_loop(
            Box::new(self.cmd),
            &forward_cmd_settings,
            Some(revert_cmd_settings),
        )
        .await?
    }
}

impl<C> CmdBuilder<C> {
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

type CmdBuilderFuture = Pin<Box<dyn Future<Output = Result<Output, Error>> + Send>>;

impl IntoFuture for CmdBuilder<RegularCmd> {
    type IntoFuture = CmdBuilderFuture;
    type Output = <CmdBuilderFuture as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

impl IntoFuture for CmdBuilder<RevertibleCmd> {
    type IntoFuture = CmdBuilderFuture;
    type Output = <CmdBuilderFuture as Future>::Output;

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
    mode: Option<CmdMode>,
}

impl Settings {
    const DEFAULT_SUDO: bool = false;
    const DEFAULT_CURRENT_DIR: Option<Cow<'static, str>> = None;
    const DEFAULT_MODE: CmdMode = CmdMode::Local;

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
        self.mode.replace(CmdMode::Remote);
        self
    }

    fn get_mode(&self) -> CmdMode {
        self.mode.unwrap_or(Self::DEFAULT_MODE)
    }
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum CmdMode {
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
        mut cmd: Box<dyn Cmd>,
        forward_cmd_settings: &Settings,
        mut revert_cmd_settings: Option<Settings>,
    ) -> Output {
        let maybe_sudo = if forward_cmd_settings.is_sudo() {
            "sudo -kSp '' "
        } else {
            ""
        };

        let mut raw_cmd = cmd.as_raw();
        let mut runnable_cmd = format!("{maybe_sudo}{raw_cmd}");
        let mut pipe_input = Vec::new();
        let mut progress_desc = "Running command";

        return loop {
            let mut password_to_cache = None;
            if forward_cmd_settings.is_sudo() {
                if let Ok(Item::Value(Value::String(password))) = kv!("admin/password").await {
                    pipe_input.push(Cow::Owned(password));
                } else {
                    let password: Secret<String> = prompt!(
                        "[sudo] Password for command {}",
                        raw_cmd
                            .split_once(' ')
                            .map(|opt| opt.0)
                            .unwrap_or(&raw_cmd)
                            .yellow()
                    )
                    .without_verification()
                    .hidden()
                    .get()?;
                    password_to_cache.replace(password.clone());
                    pipe_input.push(Cow::Owned(password.into_non_secret()));
                }
            }

            let sudo_str = sudo_string(forward_cmd_settings.is_sudo());
            let cmd_str = raw_cmd.as_str().yellow();
            debug!("{progress_desc}: {sudo_str}{cmd_str}");
            debug!("Host: this computer");

            if forward_cmd_settings.get_mode() == CmdMode::Remote {
                info!("Remote");
            }

            match exec(&runnable_cmd, &mut pipe_input, forward_cmd_settings).await {
                Ok(output) => {
                    if let Some(password) = password_to_cache {
                        kv!("admin/password").temporary().put(password).await?;
                    }

                    if let Some(revert_cmd_settings) = revert_cmd_settings {
                        cmd.on_finished(forward_cmd_settings.is_sudo(), revert_cmd_settings)
                            .await;
                    }

                    break output;
                }
                Err(Error::Failed(output)) => {
                    let cmd_only = raw_cmd.split_once(' ').map(|opt| opt.0).unwrap_or(&raw_cmd);

                    error!(
                        "The command {cmd_only} failed with exit code {}",
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

                    if revert_cmd_settings.is_some() {
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
                            if let Some(revert_cmd_settings) = revert_cmd_settings {
                                cmd.on_finished(
                                    forward_cmd_settings.is_sudo(),
                                    revert_cmd_settings,
                                )
                                .await;
                            }

                            throw!(err);
                        }
                    };

                    if [revert_modify, revert_rerun].contains(&opt) {
                        if let Some(revert_cmd_settings) = revert_cmd_settings.clone() {
                            cmd.on_revert(forward_cmd_settings.is_sudo(), revert_cmd_settings)
                                .await?;
                        }
                    }

                    if [Opt::Modify, revert_modify].contains(&opt) {
                        let mut prompt = prompt!("New command").with_initial_input(&raw_cmd);

                        if revert_cmd_settings.is_some() {
                            prompt = prompt.with_help_message(
                                "Modifying the command will make it non-revertible",
                            );
                        }

                        let new_raw_cmd = prompt.get()?;
                        if new_raw_cmd != raw_cmd {
                            raw_cmd = new_raw_cmd;
                            runnable_cmd = format!("{maybe_sudo}{raw_cmd}");
                            revert_cmd_settings.take();
                            cmd = Box::new(RegularCmd {
                                raw_forward_cmd: raw_cmd.clone().into(),
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
        raw_cmd: &str,
        pipe_input: &mut Vec<Cow<'_, str>>,
        settings: &Settings,
    ) -> Output {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", raw_cmd])
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
