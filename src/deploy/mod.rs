use k8s_openapi::api::core::v1::Node;
use kube::api::{Api, Meta};
use kube::{Client, Config};
use structopt::StructOpt;

use crate::prelude::*;

#[derive(StructOpt)]
pub struct CmdDeploy {
    #[structopt(long, short)]
    service: String,

    #[structopt(long, short, default_value = "master")]
    branch: String,
}

impl CmdDeploy {
    pub async fn run(self) -> AppResult<()> {
        status!(format!("Deploying service '{}'", self.service));

        std::env::set_var("KUBECONFIG", KUBE_DIR.join("config"));
        let config = Config::from_kubeconfig(&Default::default()).await?;
        let client = Client::new(config);

        let pods: Api<Node> = Api::all(client);
        for pod in pods.list(&Default::default()).await? {
            info!(Meta::name(&pod));
        }

        todo!()
    }
}
