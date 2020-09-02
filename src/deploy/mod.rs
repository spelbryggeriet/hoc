use structopt::StructOpt;

use crate::{build::CmdBuild, logger::Logger};

#[derive(StructOpt)]
pub(super) struct CmdDeploy {
    #[structopt(long, short)]
    service: String,
}

impl CmdDeploy {
    pub(super) async fn run(self, log: &mut Logger) -> anyhow::Result<()> {
        log.status(format!("Deploying service '{}'", self.service))?;
        let build_cmd = CmdBuild {
            service: self.service,
        };
        build_cmd.run(log).await
    }
}
