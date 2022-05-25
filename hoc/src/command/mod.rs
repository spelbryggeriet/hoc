use structopt::StructOpt;

pub use self::{deploy_node::DeployNode, prepare_cluster::PrepareCluster};

mod deploy_node;
mod prepare_cluster;
mod util;

#[derive(StructOpt)]
pub enum Command {
    DeployNode(DeployNode),
    PrepareCluster(PrepareCluster),
}
