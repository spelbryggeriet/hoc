use std::{fmt::Write, fs::File};

use anyhow::Error;
use serde::Deserialize;
use serde::Serialize;
use tinytemplate::TinyTemplate;

use crate::prelude::*;

#[throws(Error)]
pub fn run() {
    let file = open_hocfile()?;
    let hocfile = parse_hocfile(file)?;
    deploy_application(&hocfile)?;
    report(&hocfile.name);
}

#[throws(Error)]
fn open_hocfile() -> File {
    progress!("Opening hocfile");

    File::open("hocfile.yaml").context("hocfile not found in current directory")?
}

#[throws(Error)]
fn parse_hocfile(file: File) -> Hocfile {
    progress!("Parsing hocfile");

    serde_yaml::from_reader(file)?
}

#[throws(Error)]
fn deploy_application(hocfile: &Hocfile) {
    progress!("Deploying application");

    let shell = shell!().start()?;

    let mut tt = TinyTemplate::new();
    tt.add_template(
        "chart",
        include_str!("../../../config/helm/Chart.tmpl.yaml"),
    )?;
    tt.add_formatter("quote", |v, out| {
        write!(
            out,
            r#""{}""#,
            v.as_str()
                .ok_or_else(|| tinytemplate::error::Error::GenericError {
                    msg: "Expected a string value".to_owned(),
                })?
                .replace('\\', r#"\\"#)
                .replace('"', r#"\""#)
        )?;
        Ok(())
    });

    let context = ChartContext {
        name: &hocfile.name,
        app_version: &hocfile.version,
    };

    let chart = tt.render("chart", &context)?;

    let registry: String = kv!("registry/prefix").get()?.convert()?;

    shell.run(process!("tee /helm/hoc-service/Chart.yaml" < ("{chart}")))?;
    shell.run(process!(
        "helm upgrade {name} /helm/hoc-service/ --install --set ingress.host={host} --set image.repository={registry}/{image_name}",
        host = hocfile.domain,
        image_name = hocfile.image_name,
        name = hocfile.name,
    ))?;

    shell.exit()?;
}

fn report(application_name: &str) {
    info!("{application_name} has been successfully deployed");
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hocfile {
    name: String,
    version: String,
    image_name: String,
    domain: String,
}

#[derive(Serialize)]
struct ChartContext<'a> {
    name: &'a str,
    app_version: &'a str,
}
