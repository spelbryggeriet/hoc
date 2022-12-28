use std::{
    borrow::Cow,
    env,
    io::{self, Read, Write},
    net::{IpAddr, TcpStream},
    process::Stdio,
    sync::{Arc, Mutex, MutexGuard},
    thread,
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
    should_retry: bool,
}

impl ProcessBuilder {
    pub fn new(process: impl Into<Cow<'static, str>>) -> Self {
        Self {
            raw: process.into(),
            settings: Settings::new(),
            input_data: String::new(),
            success_codes: vec![0],
            revert_process: None,
            should_retry: true,
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
    fn shell_mode(mut self, outer_mode: ProcessMode) -> Self {
        self.settings.shell_mode(outer_mode);
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

    fn no_retry(mut self) -> Self {
        self.should_retry = false;
        self
    }

    #[allow(unused)]
    fn to_raw(&self) -> Cow<str> {
        if self.settings.is_sudo() {
            format!("sudo -kSp '' {}", self.raw).into()
        } else {
            Cow::Borrowed(&self.raw)
        }
    }

    #[throws(Error)]
    fn spawn(mut self) -> Process {
        self.update_settings();
        if let Some(process) = self.revert_process.as_mut() {
            process.update_settings();
        }
        self.spawn_no_settings_update("Running process")?
    }

    #[throws(Error)]
    pub fn run(self) -> Output {
        self.spawn()?.join()?
    }

    #[throws(Error)]
    fn spawn_no_settings_update(mut self, debug_desc: &str) -> Process {
        let mut password_to_cache = None;
        if self.settings.is_sudo() {
            let password = match self.settings.get_mode() {
                ProcessMode::Local => get_local_password()?,
                ProcessMode::Remote { .. } => get_remote_password()?,
                ProcessMode::Shell { mode } => match &**mode {
                    ProcessMode::Local => get_local_password()?,
                    ProcessMode::Remote { .. } => get_remote_password()?,
                    ProcessMode::Container => unreachable!(),
                    ProcessMode::Shell { .. } => unreachable!(),
                },
                ProcessMode::Container => unreachable!(), // container mode is always non-sudo
            };

            password_to_cache.replace(password.clone());
            self.input_data = password.into_non_secret() + "\n" + &self.input_data;
        }

        util::describe_process(&self.raw, self.settings.is_sudo(), debug_desc);

        match self.settings.get_mode() {
            ProcessMode::Local => trace!("Mode: local"),
            ProcessMode::Container => trace!("Mode: container"),
            ProcessMode::Remote { node_name } => trace!("Mode: remote => {node_name}"),
            ProcessMode::Shell { mode, .. } => match &**mode {
                ProcessMode::Local => trace!("Mode: local shell"),
                ProcessMode::Container => trace!("Mode: container shell"),
                ProcessMode::Remote { node_name } => trace!("Mode: remote shell => {node_name}"),
                ProcessMode::Shell { .. } => unreachable!(),
            },
        }

        match self.settings.get_mode() {
            ProcessMode::Local => self.spawn_local(password_to_cache)?,
            ProcessMode::Container => {
                const WAIT_SECONDS: u64 = 5;
                const TIMEOUT_SECONDS: u64 = 3 * 60;

                // Ensure Docker is started.
                for attempt in 1..=TIMEOUT_SECONDS / WAIT_SECONDS {
                    trace!("Checking if Docker is started");
                    let output = ProcessBuilder::new("docker stats --no-stream")
                        .local_mode()
                        .success_codes([0, 1])
                        .spawn_no_settings_update("Running process")?
                        .join()?;

                    if output.code == 0 {
                        break;
                    } else if attempt == 1 {
                        trace!("Starting Docker");
                        ProcessBuilder::new("open -a Docker")
                            .local_mode()
                            .spawn_no_settings_update("Running process")?
                            .join()?;
                    }

                    trace!("Waiting {WAIT_SECONDS} seconds");
                    spin_sleep::sleep(Duration::from_secs(WAIT_SECONDS));
                }

                self.spawn_container(password_to_cache)?
            }
            ProcessMode::Remote { node_name } => {
                let mut current_session = current_ssh_session();
                match &*current_session {
                    Some((current_node, session)) if node_name == current_node => {
                        self.spawn_remote(session, password_to_cache)?
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

                        let node_name = node_name.clone();
                        let process = self.spawn_remote(&session, password_to_cache);

                        current_session.replace((node_name, session));

                        process?
                    }
                }
            }
            ProcessMode::Shell { .. } => self.spawn_shell(password_to_cache)?,
        }
    }

    fn update_settings(&mut self) {
        let mut derived_settings = Settings::new();
        derived_settings.apply(&global_settings());
        derived_settings.apply(&self.settings);
        self.settings = derived_settings;
    }

    #[throws(Error)]
    pub fn spawn_local(self, password_to_cache: Option<Secret<String>>) -> Process {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", &*util::get_runnable_raw(&self)])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(current_dir) = self.settings.get_current_dir() {
            cmd.current_dir(&*current_dir);
        }

        let mut child = cmd.spawn()?;

        Process {
            builder: self,
            stdin: Stdin::Cmd(child.stdin.take().expect("stdin should not be taken")),
            stdout: Stdout::Cmd(child.stdout.take().expect("stdout should not be taken")),
            stderr: Stderr::Cmd(child.stderr.take().expect("stderr should not be taken")),
            handle: Handle::Cmd(child),
            password_to_cache,
        }
    }

    #[throws(Error)]
    pub fn spawn_container(self, password_to_cache: Option<Secret<String>>) -> Process {
        let raw: Cow<_> = if let Some(current_dir) = &self.settings.get_current_dir() {
            format!("cd {current_dir} ; {}", util::get_runnable_raw(&self)).into()
        } else {
            util::get_runnable_raw(&self)
        };

        let mut cmd = std::process::Command::new("docker");
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

        if !self.input_data.is_empty() {
            cmd.arg("-i");
        }

        let mut child = cmd
            .args([&container_image(), "sh", "-c", &*raw])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        Process {
            builder: self,
            stdin: Stdin::Cmd(child.stdin.take().expect("stdin should not be taken")),
            stdout: Stdout::Cmd(child.stdout.take().expect("stdout should not be taken")),
            stderr: Stderr::Cmd(child.stderr.take().expect("stderr should not be taken")),
            handle: Handle::Cmd(child),
            password_to_cache,
        }
    }

    #[throws(Error)]
    pub fn spawn_remote(
        self,
        session: &ssh2::Session,
        password_to_cache: Option<Secret<String>>,
    ) -> Process {
        let mut channel = session.channel_session()?;

        let raw: Cow<_> = if let Some(current_dir) = &self.settings.get_current_dir() {
            format!("cd {current_dir} ; {}", util::get_runnable_raw(&self)).into()
        } else {
            util::get_runnable_raw(&self)
        };

        channel.exec(&raw)?;

        let stdout = channel.stream(0);
        let stderr = channel.stderr();
        let handle = Arc::new(Mutex::new(channel));
        let stdin = Arc::clone(&handle);

        Process {
            builder: self,
            stdin: Stdin::Ssh(stdin),
            stdout: Stdout::Ssh(stdout),
            stderr: Stderr::Ssh(stderr),
            handle: Handle::Ssh(handle),
            password_to_cache,
        }
    }

    #[allow(unused)]
    #[throws(Error)]
    pub fn spawn_shell(self, password_to_cache: Option<Secret<String>>) -> Process {
        todo!()
    }
}

pub struct Process {
    builder: ProcessBuilder,
    stdin: Stdin,
    stdout: Stdout,
    stderr: Stderr,
    handle: Handle,
    password_to_cache: Option<Secret<String>>,
}

impl Process {
    #[throws(Error)]
    pub fn join(mut self) -> Output {
        let raw = self.builder.raw.clone();
        let is_sudo = self.builder.settings.is_sudo();

        let mut output = self.handle.join(
            self.stdin,
            self.stdout,
            self.stderr,
            self.builder.input_data.as_bytes(),
        )?;

        if self.builder.success_codes.contains(&output.code) {
            if let Some(password) = self.password_to_cache {
                match self.builder.settings.get_mode() {
                    ProcessMode::Local => kv!("admin/passwords/local"),
                    ProcessMode::Remote { .. } => kv!("admin/passwords/remote"),
                    ProcessMode::Shell { mode } => match &**mode {
                        ProcessMode::Local => kv!("admin/passwords/local"),
                        ProcessMode::Remote { .. } => kv!("admin/passwords/remote"),
                        ProcessMode::Container => unreachable!(),
                        ProcessMode::Shell { .. } => unreachable!(),
                    },
                    ProcessMode::Container => unreachable!(),
                }
                .temporary()
                .put(password)?;
            }

            if let Some(revert_process) = self.builder.revert_process {
                let transaction = ledger::RevertibleProcess::new(
                    self.builder.raw,
                    self.builder.settings.is_sudo(),
                    *revert_process,
                );
                Ledger::get_or_init().add(transaction);
            }
        } else {
            let program = self
                .builder
                .raw
                .split_once(' ')
                .map(|opt| opt.0)
                .unwrap_or(&self.builder.raw);

            if !self.builder.should_retry {
                throw!(Error::Failed(output));
            }

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

            if self.builder.revert_process.is_some() {
                select = select.with_option(revert_modify).with_option(revert_rerun);
            }

            let mut opt = select
                .with_option(Opt::Modify)
                .with_option(Opt::Rerun)
                .with_option(Opt::Skip)
                .get()?;

            if opt == Opt::Skip {
                warn!("Skipping to resolve process error");
            } else {
                if [revert_modify, revert_rerun].contains(&opt) {
                    if let Some(revert_process) = &self.builder.revert_process {
                        let transaction = ledger::RevertibleProcess::new(
                            self.builder.raw.clone(),
                            self.builder.settings.is_sudo(),
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
                    let mut prompt = prompt!("New process").with_initial_input(&self.builder.raw);

                    if self.builder.revert_process.is_some() {
                        prompt = prompt
                            .with_help_message("Modifying the process will make it non-revertible");
                    }

                    let new_raw_process: String = prompt.get()?;
                    if new_raw_process != self.builder.raw {
                        self.builder.raw = new_raw_process.into();
                        self.builder.revert_process.take();
                    } else {
                        opt = Opt::Rerun;
                    }
                }

                if [Opt::Rerun, revert_rerun].contains(&opt) {
                    output = self
                        .builder
                        .spawn_no_settings_update("Re-running process")?
                        .join()?;
                } else {
                    output = self
                        .builder
                        .spawn_no_settings_update("Running modified process")?
                        .join()?;
                }
            }
        }

        util::describe_process(&raw, is_sudo, "Finished process");
        output
    }
}

enum Handle {
    Cmd(std::process::Child),
    Ssh(Arc<Mutex<ssh2::Channel>>),
}

impl Handle {
    #[throws(Error)]
    fn join(self, mut stdin: Stdin, stdout: Stdout, stderr: Stderr, input_data: &[u8]) -> Output {
        if !input_data.is_empty() {
            stdin.write_all(input_data)?;
        }

        let mut output = Output::new();
        thread::scope(|s| -> Result<(), Error> {
            let stdout_handle =
                s.spawn(|| util::read_lines(stdout, |line| debug!("[stdout] {line}")));
            let stderr_handle =
                s.spawn(|| util::read_lines(stderr, |line| debug!("[stderr] {line}")));

            output.stdout = stdout_handle.join().expect(EXPECT_THREAD_NOT_POSIONED)?;
            output.stderr = stderr_handle.join().expect(EXPECT_THREAD_NOT_POSIONED)?;

            Ok(())
        })?;

        match self {
            Self::Cmd(mut child) => {
                let status = child.wait()?;
                let Some(code) = status.code() else {
                    throw!(Error::Terminated)
                };
                output.code = code;
            }
            Self::Ssh(channel) => {
                let mut channel = channel.lock().expect(EXPECT_THREAD_NOT_POSIONED);
                channel.close()?;
                channel.wait_close()?;
                output.code = channel.exit_status()?;
            }
        }

        output
    }
}

enum Stdin {
    Cmd(std::process::ChildStdin),
    Ssh(Arc<Mutex<ssh2::Channel>>),
}

impl Write for Stdin {
    #[throws(io::Error)]
    fn write(&mut self, buf: &[u8]) -> usize {
        match self {
            Self::Cmd(stdin) => stdin.write(buf)?,
            Self::Ssh(channel) => channel
                .lock()
                .expect(EXPECT_THREAD_NOT_POSIONED)
                .write(buf)?,
        }
    }

    #[throws(io::Error)]
    fn write_all(&mut self, buf: &[u8]) {
        match self {
            Self::Cmd(stdin) => stdin.write_all(buf)?,
            Self::Ssh(channel) => channel
                .lock()
                .expect(EXPECT_THREAD_NOT_POSIONED)
                .write_all(buf)?,
        }
    }

    #[throws(io::Error)]
    fn flush(&mut self) {
        match self {
            Self::Cmd(stdin) => stdin.flush()?,
            Self::Ssh(channel) => channel.lock().expect(EXPECT_THREAD_NOT_POSIONED).flush()?,
        }
    }
}

enum Stdout {
    Cmd(std::process::ChildStdout),
    Ssh(ssh2::Stream),
}

impl Read for Stdout {
    #[throws(io::Error)]
    fn read(&mut self, buf: &mut [u8]) -> usize {
        match self {
            Self::Cmd(stdout) => stdout.read(buf)?,
            Self::Ssh(stream) => stream.read(buf)?,
        }
    }
}

enum Stderr {
    Cmd(std::process::ChildStderr),
    Ssh(ssh2::Stream),
}

impl Read for Stderr {
    #[throws(io::Error)]
    fn read(&mut self, buf: &mut [u8]) -> usize {
        match self {
            Self::Cmd(stderr) => stderr.read(buf)?,
            Self::Ssh(stream) => stream.read(buf)?,
        }
    }
}

pub struct Shell<S> {
    state: S,
}

pub struct Idle {
    builder: ProcessBuilder,
}

pub struct Running {
    mode: ProcessMode,
    process: Process,
}

impl Shell<Idle> {
    pub fn new() -> Self {
        Self {
            state: Idle {
                builder: ProcessBuilder::new("sh").no_retry(),
            },
        }
    }

    #[throws(Error)]
    pub fn start(self) -> Shell<Running> {
        let mode = self.state.builder.settings.get_mode().clone();
        let process = self
            .state
            .builder
            .spawn_no_settings_update("Running process")?;

        Shell {
            state: Running { mode, process },
        }
    }
}

impl Shell<Running> {
    #[throws(Error)]
    pub fn run(&self, process: ProcessBuilder) -> Output {
        process.shell_mode(self.state.mode.clone()).run()?
    }

    #[throws(Error)]
    pub fn exit(mut self) -> Output {
        self.run(ProcessBuilder::new("exit 0"))?;
        match &mut self.state.process.stdin {
            Stdin::Ssh(channel) => channel
                .lock()
                .expect(EXPECT_THREAD_NOT_POSIONED)
                .send_eof()?,
            Stdin::Cmd(_) => (),
        }

        self.state.process.join()?
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
    const DEFAULT_MODE: ProcessMode = ProcessMode::Container;

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
        self.mode.replace(ProcessMode::Remote {
            node_name: node_name.into(),
        });
        self
    }

    fn shell_mode(&mut self, outer_mode: ProcessMode) -> &mut Self {
        self.mode.replace(ProcessMode::Shell {
            mode: Box::new(outer_mode),
        });
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
    Remote {
        node_name: Cow<'static, str>,
    },
    Shell {
        mode: Box<ProcessMode>,
    },
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
    use std::io::{BufRead, BufReader, Read};

    use super::*;

    pub fn describe_process(raw: &str, is_sudo: bool, desc: &str) {
        let sudo_str = colored_sudo_string(is_sudo);
        let process_str = raw.yellow().to_string();
        debug!("{desc}: {sudo_str}{process_str}");
    }

    pub fn colored_sudo_string(is_sudo: bool) -> Cow<'static, str> {
        if is_sudo {
            format!("{} ", "sudo".black().on_yellow()).into()
        } else {
            "".into()
        }
    }

    pub fn get_runnable_raw(process: &ProcessBuilder) -> Cow<'static, str> {
        let maybe_sudo = if process.settings.is_sudo() {
            "sudo -kSp '' "
        } else {
            ""
        };

        format!("{maybe_sudo}{}", process.raw).into()
    }

    #[throws(Error)]
    pub fn read_lines(reader: impl Read, print_line: impl Fn(&str)) -> String {
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

    pub struct RevertibleProcess {
        raw_forward_process: Cow<'static, str>,
        is_forward_process_sudo: bool,
        revert_process: ProcessBuilder,
    }

    impl RevertibleProcess {
        pub fn new(
            raw_forward_process: Cow<'static, str>,
            is_forward_process_sudo: bool,
            revert_process: ProcessBuilder,
        ) -> Self {
            RevertibleProcess {
                raw_forward_process,
                is_forward_process_sudo,
                revert_process,
            }
        }
    }

    impl Transaction for RevertibleProcess {
        fn description(&self) -> Cow<'static, str> {
            "Run process".into()
        }

        fn detail(&self) -> Cow<'static, str> {
            let sudo_str = util::colored_sudo_string(
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
