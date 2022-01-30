use structopt::StructOpt;

use configure::Configure;
use flash::Flash;

#[macro_use]
pub mod util;

mod configure;
mod flash;

#[derive(StructOpt)]
pub enum Command {
    Flash(Flash),
    Configure(Configure),
}
