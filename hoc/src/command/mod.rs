use hoclog::{info, status, LogErr};
use structopt::StructOpt;

use flash::Flash;

use crate::error;

macro_rules! _cmd {
    (
        program=$program:expr,
        args=[$($args:expr),*],
        silent=$silent:literal,
        sudo=$sudo:literal,
    ) => {{
        use ::std::process::Command;
        use ::hoclog::error;

        let program = $program;
        let args: &[String] = &[$(
            <_ as AsRef<::std::ffi::OsStr>>::as_ref(&$args).to_string_lossy().into_owned(),
        )*];

        let mut command_string = if $sudo {
            "sudo ".to_string()
        } else {
            String::new()
        };

        command_string.push_str(&program);

        for arg in args.iter() {
            command_string.push(' ');
            command_string.push_str(arg);
        }

        let runner = || {
            #[allow(unused_mut)]
            let mut command = if $sudo {
                let mut command = Command::new("sudo");
                command.args(["-p", &hoclog::LOG.create_line_prefix("Password:")]);
                command.arg(&program);
                command
            } else {
                Command::new(&program)
            };

            for arg in args {
                command.arg(arg);
            }

            let output = command
                .output()
                .log_with_context(|e| format!("Failed to run {}: {}", program, e))?;

            if !output.status.success() {
                let mut error_msg = format!("{} failed with {}", program, output.status);

                if !output.stdout.is_empty() {
                    error_msg += &format!("\n\n[stdout]\n{}", String::from_utf8_lossy(&output.stdout));
                }
                if !output.stderr.is_empty() {
                    error_msg += &format!("\n\n[stderr]\n{}", String::from_utf8_lossy(&output.stderr));
                }

                return error!(error_msg).map(|_| Default::default());
            }

            let output = String::from_utf8_lossy(
                output
                    .stdout
                    .split_at(
                        output
                            .stdout
                            .iter()
                            .position(|&b| !(b as char).is_ascii_whitespace())
                            .unwrap_or(output.stdout.len()),
                    )
                    .1
                    .split_at(
                        output.stdout.len()
                            - output
                                .stdout
                                .iter()
                                .rev()
                                .position(|&b| !(b as char).is_ascii_whitespace())
                                .unwrap_or(output.stdout.len()),
                    )
                    .0,
            )
            .into_owned();

            if !$silent {
                info!(&output);
            }

            Ok(output)
        };

        if !$silent {
            status!(("Running command: {}", command_string), runner())
        } else {
            runner()
        }
    }};
}

macro_rules! cmd {
    ($program:expr $(, $args:expr)* $(,)?) => {
        _cmd!(
            program=$program,
            args=[$($args),*],
            silent=false,
            sudo=false,
        )
    };
}

macro_rules! cmd_silent {
    ($program:expr $(, $args:expr)* $(,)?) => {
        _cmd!(
            program=$program,
            args=[$($args),*],
            silent=true,
            sudo=false,
        )
    };
}

macro_rules! sudo_cmd {
    ($program:expr $(, $args:expr)* $(,)?) => {
        _cmd!(
            program=$program,
            args=[$($args),*],
            silent=false,
            sudo=true,
        )
    };
}

mod flash;

pub fn reset_sudo_privileges() -> hoclog::Result<()> {
    cmd_silent!("sudo", "-k")?;
    Ok(())
}

#[derive(StructOpt)]
pub enum Command {
    Flash(Flash),
}
