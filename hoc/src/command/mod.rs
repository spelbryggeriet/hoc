use structopt::StructOpt;

use flash::Flash;

macro_rules! _cmd {
    ($program:expr $(, $args:expr)* $(,)? => [$output_ident:ident] $output:expr) => {{
        use ::std::process::Command;
        use ::hoclog::{bail, LogErr};

        let program = $program;
        let mut command = Command::new(&program);

        #[allow(unused_mut)]
        let mut command_string = <_ as AsRef<::std::ffi::OsStr>>::as_ref(&program)
            .to_string_lossy()
            .into_owned();

        {$(
            let arg = $args;
            command.arg(&arg);
            command_string.push(' ');
            command_string.push_str(&<_ as AsRef<::std::ffi::OsStr>>::as_ref(&arg).to_string_lossy());
        )*}

        status!(("Running command: {}", command_string), {
            let $output_ident = command
                .output()
                .log_with_context(|e| format!("Failed to run {}: {}", program, e))?;

            if !$output_ident.status.success() {
                let mut bail_msg = format!("{} failed with {}", $program, $output_ident.status);

                if !$output_ident.stdout.is_empty() {
                    bail_msg += &format!("\n\n[stdout]\n{}", String::from_utf8_lossy(&$output_ident.stdout));
                }
                if !$output_ident.stderr.is_empty() {
                    bail_msg += &format!("\n\n[stderr]\n{}", String::from_utf8_lossy(&$output_ident.stderr));
                }

                bail!(bail_msg);
            }

            $output
        })
    }};
}

macro_rules! cmd {
    ($($t:tt)*) => {
        _cmd!($($t)* => [output] {
            let output = String::from_utf8_lossy(
                output.stdout.split_at(
                    output.stdout.iter()
                        .position(|&b| !(b as char).is_ascii_whitespace())
                        .unwrap_or(output.stdout.len()),
                )
                .1
                .split_at(
                    output.stdout.len()
                        - output.stdout.iter()
                            .rev()
                            .position(|&b| !(b as char).is_ascii_whitespace())
                            .unwrap_or(output.stdout.len()),
                )
                .0,
            ).into_owned();

            info!(&output);
            output
        })
    }
}

macro_rules! cmd_silent {
    ($($t:tt)*) => { _cmd!($($t)* => [output] output.stdout) };
}

mod flash;

#[derive(StructOpt)]
pub enum Command {
    Flash(Flash),
}
