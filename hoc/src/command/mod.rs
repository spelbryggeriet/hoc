use structopt::StructOpt;

use flash::Flash;

mod flash;

#[derive(StructOpt)]
pub enum Command {
    Flash(Flash),
}
