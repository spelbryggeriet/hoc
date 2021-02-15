use k8s_openapi::api::core::v1::Node;
use kube::api::{Api, Meta};
use kube::{Client, Config};

use crate::prelude::*;

pub struct FnK8sDeploy {
    pub service: String,
    pub branch: String,
}

impl FnK8sDeploy {
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
