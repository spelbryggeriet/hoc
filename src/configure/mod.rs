mod parse;

use std::env;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::anyhow;
use ssh2::Session;
use structopt::StructOpt;

use crate::prelude::*;

macro_rules! shell_cmd {
    ($cmd:literal) => {{
        format!(include_str!(concat!("shell/", $cmd, ".sh.fmt")))
    }};

    ($cmd:literal, $($args:tt)*) => {{
        format!(include_str!(concat!("shell/", $cmd, ".sh.fmt")), $($args)*)
    }};
}

macro_rules! ssh_cmd {
    ($ssh:expr, $context:expr, $($cmd:tt)+) => {{
        let context = $context;
        $ssh.run_command(shell_cmd!($($cmd)+))
            .context(context.clone())?
            .map_err(|(exit_status, output)| anyhow!(
                "Command exited with status code {}: {}",
                exit_status,
                output
            ))
            .context(context)?;
    }};
}

enum Authentication<S, P> {
    Password(S),
    KeyBased { pub_key: P, priv_key: P },
}

#[derive(StructOpt)]
pub(super) struct CmdConfigure {
    #[structopt(long)]
    fresh: bool,

    #[structopt(long)]
    skip_dependencies: bool,

    node_name: String,
}

impl CmdConfigure {
    pub async fn run(self, context: &mut AppContext) -> AppResult<()> {
        if self.fresh {
            context.clear_node_config(&self.node_name)?;
        }

        let mut local_endpoints = self
            .get_local_endpoints()
            .context("Getting local endpoints")?;

        let default_index = local_endpoints
            .iter()
            .position(|e| {
                e.hostname
                    .as_ref()
                    .map(|v| v.contains(&self.node_name))
                    .unwrap_or_default()
            })
            .unwrap_or_default();

        let index = choose!(
            "Which endpoint do you want to configure?",
            local_endpoints.iter(),
            default_index,
        );
        let local_endpoint = local_endpoints.remove(index);

        let mut username = context
            .get_username(&self.node_name)
            .unwrap_or("pi")
            .to_string();
        let mut password = if username == "pi" {
            "raspberry".to_string()
        } else {
            input!([hidden] "Password for {}", username)
        };

        status!("Connecting to SSH host");
        let home_var = env::var("HOME")?;
        let mut ssh = SshClient::new(
            local_endpoint,
            &username,
            context.get_ssh_identity_name(&self.node_name).map_or(
                Authentication::Password(&password),
                |name| {
                    let base_path = PathBuf::from(format!("{}/.ssh", home_var));
                    Authentication::KeyBased {
                        pub_key: base_path.join(format!("{}.pub", name)),
                        priv_key: base_path.join(name),
                    }
                },
            ),
        )
        .context("Creating new SSH client")?;

        if username == "pi" {
            status!("Migrating from the pi user");

            username = input!("Choose a new username");
            context.update_username(&self.node_name, username.clone())?;

            password = input!([hidden] "Choose a new password");
            let new_password_retype = input!([hidden] "Retype the new password");
            if password != new_password_retype {
                anyhow::bail!("Passwords doesn't match");
            }

            status!("Creating new user");
            ssh_cmd!(
                ssh,
                "Creating new user",
                "add_user",
                username = username,
                password = password
            );

            status!("Reconnecting with new credentials");
            ssh.reconnect(&username, Authentication::Password::<_, PathBuf>(&password))
                .context("Reconnecting SSH client with new credentials")?;

            status!("Deleting pi user");
            ssh_cmd!(
                ssh,
                "Deleting pi user",
                "delete_pi_user",
                password = password
            );
        }

        if !ssh.path_exists(format!("/home/{}/.ssh/authorized_keys", username))? {
            status!("Initializing SSH key-based authorization");

            let mut identities = self.get_ssh_identities()?;
            let default_index = identities
                .iter()
                .position(|i| {
                    i.to_str()
                        .map(|v| v.ends_with("id_rsa.pub"))
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            let index = choose!(
                "Which SSH public identity file would you like to use",
                identities.iter().map(|i| i.to_string_lossy()),
                default_index,
            );
            let identity_path = identities.remove(index);

            context.update_ssh_identity_name(
                &self.node_name,
                identity_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
                    .context("Path could not be converted to UTF-8")?,
            )?;

            status!("Configuring SSH key-based authentication");
            ssh_cmd!(
                ssh,
                "Configuring SSH key-based authentication",
                "configure_ssh",
                username = username,
                password = password
            );

            ssh.send_file(identity_path, 0o600)
                .context("Copying SSH public identity file")?;
        }

        if !self.skip_dependencies {
            status!("Installing apt packages");
            for package_name in &[
                "openssh-server",
                "ufw",
                "pkg-config",
                "libssl-dev",
                "libx11-dev",
                "libxcb-composite0-dev",
                "docker.io",
            ] {
                let msg = format!("Installing {}", package_name);
                status!(&msg);
                ssh_cmd!(
                    ssh,
                    msg,
                    "install_apt_package",
                    password = password,
                    package_name = package_name,
                );
            }

            status!("Installing Rust");
            ssh_cmd!(ssh, "Installing Rust", "install_rust");

            status!("Installing Rust packages");
            for package_name in &["nu"] {
                let msg = format!("Installing {}", package_name);
                status!(&msg);
                ssh_cmd!(
                    ssh,
                    msg,
                    "install_rust_package",
                    package_name = package_name,
                );
            }
        }

        status!("Configuring cron jobs");
        ssh_cmd!(
            ssh,
            "Configuring cron jobs",
            "configure_cron_jobs",
            password = password
        );

        status!("Configuring firewall");
        ssh_cmd!(
            ssh,
            "Configuring firewall",
            "configure_firewall",
            password = password
        );

        status!("Configuring hostname");
        ssh_cmd!(
            ssh,
            "Configuring hostname",
            "configure_hostname",
            password = password,
            hostname = self.node_name,
        );

        status!("Rebooting the node");
        ssh_cmd!(ssh, "Rebooting the node", "reboot", password = password,);

        Ok(())
    }

    fn get_local_endpoints(&self) -> AppResult<Vec<LocalEndpoint>> {
        let stdout = if cfg!(target_os = "macos") {
            Command::new("arp")
                .arg("-a")
                .output()
                .context("Executing arp")?
                .stdout
        } else if cfg!(target_os = "linux") {
            unimplemented!();
        } else {
            anyhow::bail!("Windows not supported");
        };

        let output = String::from_utf8(stdout).context("Converting stdout to UTF-8")?;
        let (_, local_endpoints) = parse::arp_output(&output)
            .map_err(|e| anyhow!(e.to_string()))
            .context("Parsing arp output")?;

        Ok(local_endpoints)
    }

    fn get_ssh_identities(&self) -> AppResult<Vec<PathBuf>> {
        let base_path = PathBuf::from(format!("{}/.ssh", env::var("HOME")?));
        let stdout = Command::new("ls")
            .arg("-1")
            .arg(&base_path)
            .output()
            .context("Executing command ls")?
            .stdout;

        let output = String::from_utf8(stdout).context("Converting stderr to UTF-8")?;
        let identities: Vec<_> = output
            .split('\n')
            .filter(|s| s.ends_with(".pub"))
            .map(|s| base_path.join(s))
            .collect();

        if identities.is_empty() {
            anyhow::bail!("No SSH identities found");
        }

        Ok(identities)
    }
}

fn create_session(
    local_endpoint: &LocalEndpoint,
    username: &str,
    auth: Authentication<impl AsRef<str>, impl AsRef<Path>>,
) -> AppResult<Session> {
    let port = 22;
    let stream = TcpStream::connect(if let Some(hostname) = local_endpoint.hostname.as_ref() {
        format!("{}:{}", hostname, port)
    } else {
        format!("{}:{}", local_endpoint.ip_address, port)
    })?;

    let mut session = Session::new()?;
    session.set_tcp_stream(stream);
    session.handshake()?;

    match auth {
        Authentication::KeyBased { pub_key, priv_key } => session.userauth_pubkey_file(
            username,
            Some(pub_key.as_ref()),
            priv_key.as_ref(),
            None,
        )?,
        Authentication::Password(password) => {
            session.userauth_password(username, password.as_ref())?
        }
    }

    Ok(session)
}

struct SshClient {
    local_endpoint: LocalEndpoint,
    session: Session,
}

impl SshClient {
    fn new(
        local_endpoint: LocalEndpoint,
        username: &str,
        auth: Authentication<impl AsRef<str>, impl AsRef<Path>>,
    ) -> AppResult<Self> {
        let session = create_session(&local_endpoint, username, auth)?;

        Ok(Self {
            local_endpoint,
            session,
        })
    }

    fn reconnect(
        &mut self,
        username: &str,
        auth: Authentication<impl AsRef<str>, impl AsRef<Path>>,
    ) -> AppResult<()> {
        let session = create_session(&self.local_endpoint, username, auth)?;
        self.session = session;

        Ok(())
    }

    fn run_command(&self, cmd: impl AsRef<str>) -> AppResult<Result<String, (i32, String)>> {
        let mut channel = self
            .session
            .channel_session()
            .context("Opening SSH channel session")?;

        channel
            .exec(cmd.as_ref())
            .context("Executing command over SSH")?;

        let mut stdout = String::new();
        let reader = BufReader::new(channel.stream(0));
        for line in reader.lines() {
            let line = line?;
            info!(line);

            if !stdout.is_empty() {
                stdout.push('\n');
            }
            stdout.push_str(&line);
        }

        let mut stderr = String::new();
        channel.stderr().read_to_string(&mut stderr)?;

        channel.close()?;
        channel.wait_close()?;

        let exit_status = channel.exit_status()?;
        if exit_status == 0 {
            Ok(Ok(stdout))
        } else {
            Ok(Err((exit_status, stderr)))
        }
    }

    fn send_file(&self, file_path: impl AsRef<Path>, mode: i32) -> AppResult<()> {
        let file_contents = fs::read(file_path)?;
        let mut remote_file = self
            .session
            .scp_send(
                Path::new(".ssh/authorized_keys"),
                mode,
                file_contents.len() as u64,
                None,
            )
            .unwrap();
        remote_file.write(&file_contents)?;

        Ok(())
    }

    fn path_exists(&self, path: impl AsRef<str>) -> AppResult<bool> {
        Ok(self
            .run_command(format!("test -e {}", path.as_ref()))
            .context("Checking if path exists")?
            .is_ok())
    }
}

struct LocalEndpoint {
    hostname: Option<String>,
    ip_address: Ipv4Addr,
    interface: String,
}

impl Display for LocalEndpoint {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        if let Some(hostname) = self.hostname.as_ref() {
            write!(
                f,
                "{} at {} connected via {}",
                hostname, self.ip_address, self.interface
            )
        } else {
            write!(f, "{} connected via {}", self.ip_address, self.interface)
        }
    }
}
