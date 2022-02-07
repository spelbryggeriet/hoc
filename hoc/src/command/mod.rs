use structopt::StructOpt;

use configure::Configure;
use flash::Flash;

mod configure;
mod flash;

#[derive(StructOpt)]
pub enum Command {
    Flash(Flash),
    Configure(Configure),
}
