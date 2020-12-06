use structopt::StructOpt;

use super::build::CmdBuild;
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
        let build_cmd = CmdBuild {
            service: self.service,
            branch: self.branch,
        };
        build_cmd.run().await
    }
}
