use structopt::StructOpt;

pub use self::{
    create_user::CreateUser, download_image::DownloadImage, init::Init,
    prepare_sd_card::PrepareSdCard,
};

mod create_user;
mod download_image;
mod init;
mod prepare_sd_card;
mod util;

#[derive(StructOpt)]
pub enum Command {
    CreateUser(CreateUser),
    DownloadImage(DownloadImage),
    PrepareSdCard(PrepareSdCard),
    Init(Init),
}
