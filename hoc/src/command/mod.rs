use structopt::StructOpt;

pub use self::{create_user::CreateUser, download_image::DownloadImage, flash::Flash, init::Init};

#[macro_use]
mod util;

mod create_user;
mod download_image;
mod flash;
mod init;

#[derive(StructOpt)]
pub enum Command {
    CreateUser(CreateUser),
    DownloadImage(DownloadImage),
    Flash(Flash),
    Init(Init),
}
