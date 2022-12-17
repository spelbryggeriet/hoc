use std::{
    borrow::Cow,
    io,
    process::Stdio,
    sync::{Mutex, MutexGuard},
};

use crossterm::style::Stylize;
use once_cell::sync::OnceCell;
use thiserror::Error;

use crate::{
    context::kv::{self, Item, Value},
    ledger::{Ledger, Transaction},
    prelude::*,
    prompt,
    util::Opt,
};

fn current_ssh_session() -> MutexGuard<'static, Option<(Cow<'static, str>, ssh2::Session)>> {
    type NodeSession = (Cow<'static, str>, ssh2::Session);

    static CURRENT_SSH_SESSION: OnceCell<Mutex<Option<NodeSession>>> = OnceCell::new();

    CURRENT_SSH_SESSION
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect(EXPECT_THREAD_NOT_POSIONED)
}

pub fn global_settings<'a>() -> MutexGuard<'a, Settings> {
    static SETTINGS: OnceCell<Mutex<Settings>> = OnceCell::new();

    SETTINGS
        .get_or_init(|| Mutex::new(Settings::new()))
        .lock()
        .expect(EXPECT_THREAD_NOT_POSIONED)
}

#[derive(Clone)]
pub struct ProcessBuilder<C> {
    handler: C,
    settings: Settings,
    success_codes: Vec<i32>,
}

trait ProcessHandler {
    fn as_raw(&self) -> String;

    fn on_finished(&self, _is_forward_process_sudo: bool, _revert_process_settings: Settings) {}

    fn on_revert(
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

            #[throws(anyhow::Error)]
            fn revert(self: Box<Self>) {
                util::run_loop(
                    Box::new(self.revert_process.handler),
                    &self.revert_process_settings,
                    None,
                    &self.revert_process.success_codes,
                )?;
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

impl ProcessHandler for RevertibleProcessHandler {
    fn as_raw(&self) -> String {
        self.raw_forward_process.clone().into_owned()
    }

    fn on_finished(&self, is_forward_process_sudo: bool, revert_process_settings: Settings) {
        let transaction = self.get_transaction(is_forward_process_sudo, revert_process_settings);
        Ledger::get_or_init().add(transaction);
    }

    #[throws(Error)]
    fn on_revert(&self, is_forward_process_sudo: bool, revert_process_settings: Settings) {
        let transaction = self.get_transaction(is_forward_process_sudo, revert_process_settings);

        info!("{}", transaction.detail());

        let opt = select!("Do you want to revert the failed command?")
            .with_options([Opt::Yes, Opt::No])
            .get()?;
        if opt == Opt::Yes {
            Box::new(transaction).revert()?;
        }
    }
}

impl ProcessBuilder<RegularProcessHandler> {
    pub fn new(process: Cow<'static, str>) -> Self {
        Self {
            handler: RegularProcessHandler {
                raw_forward_process: process,
            },
            settings: Settings::new(),
            success_codes: vec![0],
        }
    }

    pub fn _revertible(self, revert_process: Self) -> ProcessBuilder<RevertibleProcessHandler> {
        ProcessBuilder {
            handler: RevertibleProcessHandler {
                raw_forward_process: self.handler.raw_forward_process,
                revert_process,
            },
            settings: self.settings,
            success_codes: self.success_codes,
        }
    }

    #[throws(Error)]
    pub fn run(self) -> Output {
        let settings: Settings = self.get_current_settings();
        util::run_loop(Box::new(self.handler), &settings, None, &self.success_codes)?
    }
}

impl ProcessBuilder<RevertibleProcessHandler> {
    #[throws(Error)]
    pub fn run(self) -> Output {
        let forward_process_settings = self.get_current_settings();
        let revert_process_settings = self.handler.revert_process.get_current_settings();
        util::run_loop(
            Box::new(self.handler),
            &forward_process_settings,
            Some(revert_process_settings),
            &self.success_codes,
        )?
    }
}

impl<C> ProcessBuilder<C> {
    fn get_current_settings(&self) -> Settings {
        let mut derived_settings = Settings::new();
        derived_settings.apply(&global_settings());
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

    pub fn _remote_mode<S: Into<Cow<'static, str>>>(mut self, node_name: S) -> Self {
        self.settings.remote_mode(node_name);
        self
    }

    pub fn success_codes<I: IntoIterator<Item = i32>>(mut self, success_codes: I) -> Self {
        self.success_codes = success_codes.into_iter().collect();
        self
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

        if let Some(mode) = &other.mode {
            self.mode.replace(mode.clone());
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

    pub fn remote_mode<S: Into<Cow<'static, str>>>(&mut self, node_name: S) -> &mut Self {
        self.mode.replace(ProcessMode::Remote(node_name.into()));
        self
    }

    fn get_mode(&self) -> &ProcessMode {
        self.mode.as_ref().unwrap_or(&Self::DEFAULT_MODE)
    }
}

#[derive(Default, Clone)]
enum ProcessMode {
    #[default]
    Local,
    Remote(Cow<'static, str>),
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

    #[error(transparent)]
    Ssh(#[from] ssh2::Error),
}

mod util {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::{IpAddr, TcpStream},
        process::Command,
        thread,
    };

    use super::*;

    #[throws(Error)]
    pub(super) fn run_loop(
        mut process_handler: Box<dyn ProcessHandler>,
        forward_process_settings: &Settings,
        mut revert_process_settings: Option<Settings>,
        success_codes: &[i32],
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
                if let Ok(Item::Value(Value::String(password))) = kv!("admin/password").get() {
                    pipe_input.push(Cow::Owned(password));
                } else {
                    let password: Secret<String> = prompt!("[sudo] Password",)
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

            let result = match forward_process_settings.get_mode() {
                ProcessMode::Local => exec_local(
                    &runnable_process,
                    &mut pipe_input,
                    success_codes,
                    forward_process_settings,
                ),
                ProcessMode::Remote(node_name) => {
                    let mut current_session = current_ssh_session();
                    match &*current_session {
                        Some((current_node, session)) if node_name == current_node => exec_remote(
                            session,
                            &runnable_process,
                            &mut pipe_input,
                            success_codes,
                            forward_process_settings,
                        ),
                        _ => {
                            let host: IpAddr =
                                kv!("nodes/{node_name}/network/address").get()?.convert()?;
                            let port = 22;
                            let stream = TcpStream::connect(format!("{host}:{port}"))?;

                            let mut session = ssh2::Session::new()?;
                            session.set_tcp_stream(stream);
                            session.handshake()?;

                            let admin_username: String = kv!("admin/username").get()?.convert()?;
                            let (_, pub_key_path) = files!("admin/ssh/pub").get()?;
                            let (_, priv_key_path) = files!("admin/ssh/priv").get()?;
                            let password: Cow<str> = if let Some(password) = &password_to_cache {
                                Cow::Borrowed(&**password)
                            } else {
                                Cow::Owned(kv!("admin/password").get()?.convert()?)
                            };

                            session.userauth_pubkey_file(
                                &admin_username,
                                Some(&pub_key_path),
                                &priv_key_path,
                                Some(&password),
                            )?;

                            let result = exec_remote(
                                &session,
                                &runnable_process,
                                &mut pipe_input,
                                success_codes,
                                forward_process_settings,
                            );

                            current_session.replace((node_name.clone(), session));

                            result
                        }
                    }
                }
            };

            match result {
                Ok(output) => {
                    if let Some(password) = password_to_cache {
                        kv!("admin/password").temporary().put(password)?;
                    }

                    if let Some(revert_process_settings) = revert_process_settings {
                        process_handler.on_finished(
                            forward_process_settings.is_sudo(),
                            revert_process_settings,
                        );
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
                        for line in output.stdout.lines() {
                            info!("[stdout] {line}");
                        }
                    }
                    if !output.stderr.is_empty() {
                        for line in output.stderr.lines() {
                            info!("[err] {line}");
                        }
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
                                process_handler.on_finished(
                                    forward_process_settings.is_sudo(),
                                    revert_process_settings,
                                );
                            }

                            throw!(err);
                        }
                    };

                    if [revert_modify, revert_rerun].contains(&opt) {
                        if let Some(revert_process_settings) = revert_process_settings.clone() {
                            process_handler.on_revert(
                                forward_process_settings.is_sudo(),
                                revert_process_settings,
                            )?;
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
    fn exec_local(
        raw_process: &str,
        pipe_input: &mut Vec<Cow<'_, str>>,
        success_codes: &[i32],
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
                stdin.write_all(input.as_bytes())?;
                stdin.write_all(b"\n")?;
            }
        }

        let mut output = Output::new();
        thread::scope(|s| -> Result<(), Error> {
            let stdout_handle = s.spawn(|| {
                read_lines(child.stdout.take().expect("stdout should exist"), |line| {
                    debug!("[stdout] {line}")
                })
            });
            let stderr_handle = s.spawn(|| {
                read_lines(child.stderr.take().expect("stderr should exist"), |line| {
                    debug!("[stderr] {line}")
                })
            });

            output.stdout = stdout_handle.join().expect(EXPECT_THREAD_NOT_POSIONED)?;
            output.stderr = stderr_handle.join().expect(EXPECT_THREAD_NOT_POSIONED)?;

            Ok(())
        })?;

        let status = child.wait()?;

        let Some(code) = status.code() else {
            throw!(Error::Terminated)
        };
        output.code = code;

        if success_codes.contains(&output.code) {
            output
        } else {
            throw!(Error::Failed(output))
        }
    }

    #[throws(Error)]
    pub fn exec_remote(
        session: &ssh2::Session,
        raw_process: &str,
        pipe_input: &mut Vec<Cow<'_, str>>,
        success_codes: &[i32],
        settings: &Settings,
    ) -> Output {
        let mut channel = session.channel_session()?;

        let raw_process: Cow<_> = if let Some(current_dir) = &settings.get_current_dir() {
            format!("cd {current_dir} ; {raw_process}").into()
        } else {
            raw_process.into()
        };

        channel.exec(&raw_process)?;

        if !pipe_input.is_empty() {
            for input in pipe_input {
                channel.write_all(input.as_bytes())?;
                channel.write_all(b"\n")?;
            }

            channel.send_eof()?;
        }

        let mut output = Output::new();
        thread::scope(|s| -> Result<(), Error> {
            let stdout_handle =
                s.spawn(|| read_lines(channel.stream(0), |line| debug!("[stdout] {line}")));
            let stderr_handle =
                s.spawn(|| read_lines(channel.stderr(), |line| debug!("[stderr] {line}")));

            output.stdout = stdout_handle.join().expect(EXPECT_THREAD_NOT_POSIONED)?;
            output.stderr = stderr_handle.join().expect(EXPECT_THREAD_NOT_POSIONED)?;

            Ok(())
        })?;

        channel.close()?;
        channel.wait_close()?;

        output.code = channel.exit_status()?;

        if success_codes.contains(&output.code) {
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
    fn read_lines(reader: impl Read, print_line: impl Fn(&str)) -> String {
        let lines = BufReader::new(reader).lines();
        let mut out = String::new();

        for line in lines {
            let line = line?;
            print_line(&line);
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&line);
        }

        out
    }
}
