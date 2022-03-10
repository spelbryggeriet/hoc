use structopt::StructOpt;

pub use self::{
    configure::Configure, create_user::CreateUser, download_image::DownloadImage, flash::Flash,
};

#[macro_use]
mod util;

mod configure;
mod create_user;
mod download_image;
mod flash;

#[derive(StructOpt)]
pub enum Command {
    CreateUser(CreateUser),
    DownloadImage(DownloadImage),
    Flash(Flash),
    Configure(Configure),
}
