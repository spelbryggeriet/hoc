use structopt::StructOpt;

use self::{configure::Configure, create_user::CreateUser, flash::Flash};

mod configure;
mod create_user;
mod flash;

#[derive(StructOpt)]
pub enum Command {
    CreateUser(CreateUser),
    Flash(Flash),
    Configure(Configure),
}
