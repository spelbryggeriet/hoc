use clap::Parser;
use env_logger::Env;

macro_rules! concat_const {
    ($first_segment:expr $(, $segments:expr)* $(,)?) => {{
        const __LEN: usize = $first_segment.len() $(+ $segments.len())*;

        const fn __copy_slice(
            input: &[u8],
            mut output: [u8; __LEN],
            offset: usize,
        ) -> (usize, [u8; __LEN]) {
            let mut index = 0;
            loop {
                output[offset + index] = input[index];
                index += 1;
                if index == input.len() {
                    break;
                }
            }
            (index + offset, output)
        }

        static mut __OUT: [u8; __LEN] = [0u8; __LEN];

        // SAFETY: `__OUT` is bound to the current scope, and is thus only accessible in the current thread.
        unsafe {
            let mut __offset = 0;
            (__offset, __OUT) = __copy_slice($first_segment.as_bytes(), __OUT, __offset);
            $(
            (__offset, __OUT) = __copy_slice($segments.as_bytes(), __OUT, __offset);
            )*
            ::std::str::from_utf8(&__OUT).unwrap_unchecked()
        }
    }};
}

macro_rules! arg_context {
    ($($field:ident $(= $default:literal)?
        -> $help:literal
      $(-> $long_help:literal)?),+ $(,)?
    ) => {
        mod default {
            $(
            $(
            pub fn $field() -> &'static str {
                $default
            }
            )?
            )+
        }

        mod help {
            $(
            mod $field {
                pub const DEFAULT: (bool, &'static str) = {
                    #[allow(unused_variables)]
                    let val = (false, "");
                    $(
                    let val = (true, $default);
                    )?
                    val
                };
            }
            )+

            $(
            pub fn $field() -> &'static str {
                if $field::DEFAULT.0 {
                    concat_const!($help, " [default: ", $field::DEFAULT.1, "]")
                } else {
                    $help
                }
            }

            $(
            pub mod long {
                pub fn $field() -> &'static str {
                    if super::$field::DEFAULT.0 {
                        concat_const!(
                            $help,
                            "\n\n",
                            $long_help,
                            "\n\n[default: ",
                            super::$field::DEFAULT.1,
                            "]",
                        )
                    } else {
                        concat_const!($help, "\n\n", $long_help)
                    }
                }
            }
            )?
            )+
        }
    };
}

macro_rules! arg_get {
    ($self:ident, $field:ident $(,)?) => {
        $crate::prompt::Prompt::get($self.$field, stringify!($field))?
    };
}

macro_rules! arg_get_or {
    ($self:ident, $field:ident $(,)?) => {
        $crate::prompt::Prompt::get_or($self.$field, stringify!($field), default::$field())?
    };
}

#[macro_use]
mod prompt;

mod cidr;
mod init;
mod prelude;

use prelude::*;

#[derive(Parser)]
struct App {
    #[clap(subcommand)]
    command: Command,
}

impl App {
    #[throws(Error)]
    fn run() {
        let app = Self::from_args();
        match app.command {
            Command::Init(init_command) => init_command.run()?,
            _ => (),
        }
    }
}

#[derive(Parser)]
enum Command {
    Deploy(DeployCommand),
    Init(init::Command),
    Node(NodeCommand),
    SdCard(SdCardCommand),
}

/// Deploy an application
#[derive(Parser)]
struct DeployCommand {}

/// Manage a node
#[derive(Parser)]
struct NodeCommand {}

/// Manage an SD card
#[derive(Parser)]
struct SdCardCommand {}

const LOWEST_DEFAULT_LEVEL: &'static str = if cfg!(debug_assertions) {
    "debug"
} else {
    "info"
};

#[throws(Error)]
fn main() {
    env_logger::Builder::from_env(Env::default().default_filter_or(LOWEST_DEFAULT_LEVEL)).init();
    App::run()?;
}
