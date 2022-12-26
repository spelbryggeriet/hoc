use std::{
    borrow::Cow,
    env, io,
    net::{IpAddr, TcpStream},
    process::Stdio,
    sync::{Mutex, MutexGuard},
    time::Duration,
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

#[throws(Error)]
pub fn get_local_password() -> Secret<String> {
    if let Ok(Item::Value(Value::String(password))) = kv!("admin/passwords/local").get() {
        Secret::new(password)
    } else {
        prompt!("[sudo] Password")
            .without_verification()
            .hidden()
            .get()?
    }
}

#[throws(Error)]
pub fn get_remote_password() -> Secret<String> {
    if let Ok(Item::Value(Value::String(password))) = kv!("admin/passwords/remote").get() {
        Secret::new(password)
    } else {
        prompt!("[remote] Administrator password")
            .without_verification()
            .hidden()
            .get()?
    }
}

pub fn global_settings<'a>() -> MutexGuard<'a, Settings> {
    static SETTINGS: OnceCell<Mutex<Settings>> = OnceCell::new();

    SETTINGS
        .get_or_init(|| Mutex::new(Settings::new()))
        .lock()
        .expect(EXPECT_THREAD_NOT_POSIONED)
}

fn container_image() -> String {
    env::var("HOC_IMAGE").unwrap_or_else(|_| "ghcr.io/spelbryggeriet/hoc-runtime".to_owned())
        + ":"
        + env!("CARGO_PKG_VERSION")
}

#[derive(Clone)]
#[must_use]
pub struct ProcessBuilder {
    raw: Cow<'static, str>,
    settings: Settings,
    input_data: String,
    success_codes: Vec<i32>,
    revert_process: Option<Box<Self>>,
}

impl ProcessBuilder {
    pub fn new(process: impl Into<Cow<'static, str>>) -> Self {
        Self {
            raw: process.into(),
            settings: Settings::new(),
            input_data: String::new(),
            success_codes: vec![0],
            revert_process: None,
        }
    }

    pub fn revertible(mut self, revert_process: Self) -> Self {
        self.revert_process.replace(Box::new(revert_process));
        self
    }

    pub fn sudo(mut self) -> Self {
        self.settings.sudo();
        self
    }

    pub fn current_dir<P: Into<Cow<'static, str>>>(mut self, current_dir: P) -> Self {
        self.settings.current_dir(current_dir);
        self
    }

    #[allow(unused)]
    pub fn local_mode(mut self) -> Self {
        self.settings.local_mode();
        self
    }

    #[allow(unused)]
    pub fn container_mode(mut self) -> Self {
        self.settings.container_mode();
        self
    }

    #[allow(unused)]
    pub fn remote_mode<S: Into<Cow<'static, str>>>(mut self, node_name: S) -> Self {
        self.settings.remote_mode(node_name);
        self
    }

    pub fn write_stdin<S: AsRef<str> + ?Sized>(mut self, input: &S) -> Self {
        self.input_data.push_str(input.as_ref());
        self
    }

    pub fn success_codes<I: IntoIterator<Item = i32>>(mut self, success_codes: I) -> Self {
        self.success_codes = success_codes.into_iter().collect();
        self
    }

    #[throws(Error)]
    #[allow(unused)]
    pub fn run(mut self) -> Output {
        self.update_settings();
        if let Some(process) = self.revert_process.as_mut() {
            process.update_settings();
        }
        self.run_no_settings_update()?
    }

    #[throws(Error)]
    fn run_no_settings_update(mut self) -> Output {
        let maybe_sudo = if self.settings.is_sudo() {
            "sudo -kSp '' "
        } else {
            ""
        };

        let mut original_raw = self.raw;
        self.raw = format!("{maybe_sudo}{original_raw}").into();
        let mut progress_desc = "Running process";
        let sudo_str = util::sudo_string(self.settings.is_sudo());
        let process_str = original_raw.yellow().to_string();

        let result = loop {
            let mut password_to_cache = None;
            if self.settings.is_sudo() {
                let password = match self.settings.get_mode() {
                    ProcessMode::Local => get_local_password()?,
                    ProcessMode::Remote(_) => get_remote_password()?,
                    ProcessMode::Container => unreachable!(),
                };
                password_to_cache.replace(password.clone());
                self.input_data = password.into_non_secret() + "\n" + &self.input_data;
            }

            debug!("{progress_desc}: {sudo_str}{process_str}");
            match self.settings.get_mode() {
                ProcessMode::Local => debug!("Mode: local"),
                ProcessMode::Container => debug!("Mode: container"),
                ProcessMode::Remote(node) => debug!("Mode: remote => {node}"),
            }

            let result = match self.settings.get_mode() {
                ProcessMode::Local => util::exec_local_or_container(&self),
                ProcessMode::Container => {
                    const WAIT_SECONDS: u64 = 5;
                    const TIMEOUT_SECONDS: u64 = 3 * 60;

                    // Ensure Docker is started.
                    for attempt in 1..=TIMEOUT_SECONDS / WAIT_SECONDS {
                        let output = ProcessBuilder::new("docker stats --no-stream")
                            .success_codes([0, 1])
                            .run_no_settings_update()?;

                        if output.code == 0 {
                            break;
                        } else if attempt == 1 {
                            ProcessBuilder::new("open -a Docker").run_no_settings_update()?;
                        }

                        debug!("Waiting {WAIT_SECONDS} seconds");
                        spin_sleep::sleep(Duration::from_secs(WAIT_SECONDS));
                    }

                    util::exec_local_or_container(&self)
                }
                ProcessMode::Remote(node_name) => {
                    let mut current_session = current_ssh_session();
                    match &*current_session {
                        Some((current_node, session)) if node_name == current_node => {
                            util::exec_remote(session, &self)
                        }
                        _ => {
                            let host: IpAddr =
                                kv!("nodes/{node_name}/network/address").get()?.convert()?;
                            let port = 22;
                            let stream = TcpStream::connect(format!("{host}:{port}"))?;

                            let mut session = ssh2::Session::new()?;
                            session.set_tcp_stream(stream);
                            session.handshake()?;

                            let admin_username: String = kv!("admin/username").get()?.convert()?;
                            let pub_key_file = files!("admin/ssh/pub").get()?;
                            let priv_key_file = files!("admin/ssh/priv").get()?;
                            let password = get_remote_password()?;
                            password_to_cache.replace(password.clone());

                            session.userauth_pubkey_file(
                                &admin_username,
                                Some(&pub_key_file.local_path),
                                &priv_key_file.local_path,
                                Some(&password.into_non_secret()),
                            )?;

                            let result = util::exec_remote(&session, &self);

                            current_session.replace((node_name.clone(), session));

                            result
                        }
                    }
                }
            };

            match result {
                Ok(output) => {
                    if let Some(password) = password_to_cache {
                        match self.settings.get_mode() {
                            ProcessMode::Local => kv!("admin/passwords/local"),
                            ProcessMode::Remote(_) => kv!("admin/passwords/remote"),
                            ProcessMode::Container => unreachable!(),
                        }
                        .temporary()
                        .put(password)?;
                    }

                    if let Some(revert_process) = self.revert_process {
                        let transaction = ledger::RevertibleTransaction::new(
                            original_raw,
                            self.settings.is_sudo(),
                            *revert_process,
                        );
                        Ledger::get_or_init().add(transaction);
                    }

                    break output;
                }
                Err(Error::Failed(output)) => {
                    let program = original_raw
                        .split_once(' ')
                        .map(|opt| opt.0)
                        .unwrap_or(&original_raw);

                    error!(
                        "The program {program} failed with exit code {}",
                        output.code,
                    );

                    let stdout = output.stdout.trim();
                    if !stdout.is_empty() {
                        for line in stdout.lines() {
                            info!("[stdout] {line}");
                        }
                    }

                    let stderr = output.stderr.trim();
                    if !stderr.is_empty() {
                        for line in stderr.lines() {
                            info!("[stderr] {line}");
                        }
                    }

                    let revert_modify = Opt::Custom("Revert and modify");
                    let revert_rerun = Opt::Custom("Revert and rerun");
                    let mut select = select!("How do you want to resolve the process error?");

                    if self.revert_process.is_some() {
                        select = select.with_option(revert_modify).with_option(revert_rerun);
                    }

                    let mut opt = select
                        .with_option(Opt::Modify)
                        .with_option(Opt::Rerun)
                        .with_option(Opt::Skip)
                        .get()?;

                    if [revert_modify, revert_rerun].contains(&opt) {
                        if let Some(revert_process) = &self.revert_process {
                            let transaction = ledger::RevertibleTransaction::new(
                                original_raw.clone(),
                                self.settings.is_sudo(),
                                *revert_process.clone(),
                            );

                            info!("{}", transaction.detail());

                            let opt = select!("Do you want to revert the failed process?")
                                .with_options([Opt::Yes, Opt::No])
                                .get()?;
                            if opt == Opt::Yes {
                                Box::new(transaction).revert()?;
                            }
                        }
                    }

                    if [Opt::Modify, revert_modify].contains(&opt) {
                        let mut prompt = prompt!("New process").with_initial_input(&original_raw);

                        if self.revert_process.is_some() {
                            prompt = prompt.with_help_message(
                                "Modifying the process will make it non-revertible",
                            );
                        }

                        let new_raw_process: String = prompt.get()?;
                        if new_raw_process != original_raw {
                            original_raw = new_raw_process.into();
                            self.raw = format!("{maybe_sudo}{original_raw}").into();
                            self.revert_process.take();
                        } else {
                            opt = Opt::Rerun;
                        }
                    } else if opt == Opt::Skip {
                        warn!("Skipping to resolve process error");
                        break output;
                    }

                    progress_desc = if opt == Opt::Rerun {
                        "Re-running process"
                    } else {
                        "Running modified process"
                    };
                }
                Err(err) => throw!(err),
            }
        };

        debug!("Finished process: {sudo_str}{process_str}");
        result
    }

    fn update_settings(&mut self) {
        let mut derived_settings = Settings::new();
        derived_settings.apply(&global_settings());
        derived_settings.apply(&self.settings);
        self.settings = derived_settings;
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
        if self.get_mode() != &ProcessMode::Container {
            self.sudo.replace(true);
        }
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

    pub fn local_mode(&mut self) -> &mut Self {
        self.mode.replace(ProcessMode::Local);
        self
    }

    #[allow(unused)]
    pub fn container_mode(&mut self) -> &mut Self {
        self.mode.replace(ProcessMode::Container);
        if let Some(s) = self.sudo.as_mut() {
            *s = false;
        }
        self
    }

    pub fn remote_mode<S: Into<Cow<'static, str>>>(&mut self, node_name: S) -> &mut Self {
        self.mode.replace(ProcessMode::Remote(node_name.into()));
        self
    }

    fn get_mode(&self) -> &ProcessMode {
        self.mode.as_ref().unwrap_or(&Self::DEFAULT_MODE)
    }
}

#[derive(Default, Clone, PartialEq, Eq)]
enum ProcessMode {
    Local,
    #[default]
    Container,
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
        process::Command,
        thread,
    };

    use super::*;

    #[throws(Error)]
    pub fn exec_local_or_container(process: &ProcessBuilder) -> Output {
        let in_container = process.settings.get_mode() == &ProcessMode::Container;
        let raw: Cow<_> = if in_container {
            if let Some(current_dir) = &process.settings.get_current_dir() {
                format!("cd {current_dir} ; {}", process.raw).into()
            } else {
                Cow::Borrowed(&process.raw)
            }
        } else {
            Cow::Borrowed(&process.raw)
        };

        let mut cmd = if in_container {
            let mut cmd = Command::new("docker");
            cmd.args([
                "run",
                "--mount",
                &format!(
                    "type=bind,source={},target={}",
                    crate::local_files_dir().to_string_lossy(),
                    crate::container_files_dir().to_string_lossy(),
                ),
                "--mount",
                &format!(
                    "type=bind,source={},target={}",
                    crate::local_cache_dir().to_string_lossy(),
                    crate::container_cache_dir().to_string_lossy(),
                ),
                "--mount",
                &format!(
                    "type=bind,source={},target={}",
                    crate::local_temp_dir().to_string_lossy(),
                    crate::container_temp_dir().to_string_lossy(),
                ),
                "--mount",
                &format!(
                    "type=bind,source={},target={}",
                    crate::local_source_dir().to_string_lossy(),
                    crate::container_source_dir().to_string_lossy(),
                ),
            ]);
            if !process.input_data.is_empty() {
                cmd.arg("-i");
            }
            cmd.args([&container_image(), "sh"]);
            cmd
        } else {
            Command::new("sh")
        };
        cmd.args(["-c", &*raw])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn()?;

        if !process.input_data.is_empty() {
            let mut stdin = child.stdin.take().expect("stdin should not be taken");
            stdin.write_all(process.input_data.as_bytes())?;
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

        if process.success_codes.contains(&output.code) {
            output
        } else {
            throw!(Error::Failed(output))
        }
    }

    #[throws(Error)]
    pub fn exec_remote(session: &ssh2::Session, process: &ProcessBuilder) -> Output {
        let mut channel = session.channel_session()?;

        let raw: Cow<_> = if let Some(current_dir) = &process.settings.get_current_dir() {
            format!("cd {current_dir} ; {}", process.raw).into()
        } else {
            Cow::Borrowed(&process.raw)
        };

        channel.exec(&raw)?;

        if !process.input_data.is_empty() {
            channel.write_all(process.input_data.as_bytes())?;
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

        if process.success_codes.contains(&output.code) {
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
        let mut buf_reader = BufReader::new(reader);
        let mut out = String::new();

        loop {
            let mut line = String::new();
            let written = buf_reader.read_line(&mut line)?;
            if written == 0 {
                break;
            }

            print_line(line.trim_end_matches('\n'));
            out.push_str(&line);
        }

        out
    }
}

mod ledger {
    use super::*;

    pub struct RevertibleTransaction {
        raw_forward_process: Cow<'static, str>,
        is_forward_process_sudo: bool,
        revert_process: ProcessBuilder,
    }

    impl RevertibleTransaction {
        pub fn new(
            raw_forward_process: Cow<'static, str>,
            is_forward_process_sudo: bool,
            revert_process: ProcessBuilder,
        ) -> Self {
            RevertibleTransaction {
                raw_forward_process,
                is_forward_process_sudo,
                revert_process,
            }
        }
    }

    impl Transaction for RevertibleTransaction {
        fn description(&self) -> Cow<'static, str> {
            "Run process".into()
        }

        fn detail(&self) -> Cow<'static, str> {
            let sudo_str = util::sudo_string(
                self.is_forward_process_sudo || self.revert_process.settings.is_sudo(),
            );

            format!(
                "Command to revert: {}{}\nCommand used to revert: {}{}",
                self.is_forward_process_sudo
                    .then_some(&*sudo_str)
                    .unwrap_or_default(),
                self.raw_forward_process.yellow(),
                self.revert_process
                    .settings
                    .is_sudo()
                    .then_some(&*sudo_str)
                    .unwrap_or_default(),
                self.revert_process.raw.yellow(),
            )
            .into()
        }

        #[throws(anyhow::Error)]
        fn revert(self: Box<Self>) {
            self.revert_process.run()?;
        }
    }
}
