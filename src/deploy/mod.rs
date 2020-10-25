use structopt::StructOpt;

use super::build::CmdBuild;
use crate::prelude::*;

#[derive(StructOpt)]
pub struct CmdDeploy {
    #[structopt(long, short)]
    service: String,
}

impl CmdDeploy {
    pub async fn run(self) -> AppResult<()> {
        status!(format!("Deploying service '{}'", self.service));
        let build_cmd = CmdBuild {
            service: self.service,
        };
        build_cmd.run().await
    }
}
