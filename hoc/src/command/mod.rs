use std::process;

use hoclog::{bail, LogErr};
use structopt::StructOpt;

use flash::Flash;

mod flash;

#[derive(StructOpt)]
pub enum Command {
    Flash(Flash),
}

fn run_system_command(cmd: &mut process::Command) -> hoclog::Result<Vec<u8>> {
    let output = cmd
        .output()
        .log_with_context(|e| format!("Failed to run command: {}", e))?;

    if !output.status.success() {
        let mut bail_msg = format!("Command failed with {}", output.status);

        if !output.stdout.is_empty() {
            bail_msg += &format!("\n\n[stdout]\n{}", String::from_utf8_lossy(&output.stdout));
        }
        if !output.stderr.is_empty() {
            bail_msg += &format!("\n\n[stderr]\n{}", String::from_utf8_lossy(&output.stderr));
        }

        bail!(bail_msg);
    }

    Ok(output.stdout)
}
