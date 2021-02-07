use bollard::{auth::DockerCredentials, image::PushImageOptions, Docker};
use futures::stream::StreamExt;
use structopt::StructOpt;

use crate::prelude::*;
use crate::service;

#[derive(StructOpt)]
pub struct CmdPublish {
    #[structopt(long, short)]
    pub service: String,

    #[structopt(long, short, default_value = "master")]
    pub branch: String,
}

impl CmdPublish {
    pub async fn run(self) -> AppResult<()> {
        let docker = Docker::connect_with_unix_defaults()?;

        let dir = tempfile::tempdir().context("Creating temporary directory")?;

        let repo = service::clone_repo(&self.service, &self.branch, dir.path())?;
        let ci_config = service::ci::get_config(&repo)?;

        let registry = format!("https://{}", service::REGISTRY_DOMAIN);
        status!("Pushing images into {}", registry);

        let username = input!("Username");
        let password = hidden_input!("Password");
        let credentials = DockerCredentials {
            username: Some(username),
            password: Some(password),
            serveraddress: Some(registry),
            ..Default::default()
        };

        for tag in ci_config.get_tags() {
            let push_options = PushImageOptions { tag };
            let mut stream = docker.push_image(
                &service::image_name(&self.service),
                Some(push_options),
                Some(credentials.clone()),
            );

            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(chunk) => {
                        if let Some(docker_status) = chunk.status {
                            info!(docker_status);
                        }
                    }
                    Err(e) => error!("{}", e),
                }
            }
        }

        Ok(())
    }
}
