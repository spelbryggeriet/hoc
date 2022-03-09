use structopt::StructOpt;

pub use self::{
    configure::Configure, create_user::CreateUser, download_os_image::DownloadOsImage, flash::Flash,
};

#[macro_use]
mod util;

mod configure;
mod create_user;
mod download_os_image;
mod flash;

#[derive(StructOpt)]
pub enum Command {
    CreateUser(CreateUser),
    DownloadOsImage(DownloadOsImage),
    Flash(Flash),
    Configure(Configure),
}
