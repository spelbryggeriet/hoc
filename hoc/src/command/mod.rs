use hoc_core::cmd_macros;
use structopt::StructOpt;

pub use self::{
    create_user::CreateUser, download_image::DownloadImage, init::Init,
    prepare_sd_card::PrepareSdCard,
};

cmd_macros!(
    adduser,
    cat,
    chmod,
    chpasswd,
    cmd_file => "file",
    dd,
    df,
    deluser,
    diskutil,
    mkdir,
    pkill,
    rm,
    sed,
    sshd,
    sync,
    systemctl,
    tee,
    test,
    usermod,
);

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
