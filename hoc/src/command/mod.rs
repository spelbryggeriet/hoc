use structopt::StructOpt;

pub use self::{
    download_image::DownloadImage, init::Init, prepare_cluster::PrepareCluster,
    prepare_sd_card::PrepareSdCard,
};

mod download_image;
mod init;
mod prepare_cluster;
mod prepare_sd_card;
mod util;

#[derive(StructOpt)]
pub enum Command {
    DownloadImage(DownloadImage),
    Init(Init),
    PrepareCluster(PrepareCluster),
    PrepareSdCard(PrepareSdCard),
}
