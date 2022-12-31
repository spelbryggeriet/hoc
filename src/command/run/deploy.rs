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
    File::open("hocfile.yaml").context("hocfile not found in current directory")?
}

#[throws(Error)]
fn parse_hocfile(file: File) -> Hocfile {
    serde_yaml::from_reader(file)?
}

#[throws(Error)]
fn deploy_application(hocfile: &Hocfile) {
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
        "helm install {name} /helm/hoc-service/ --set image.repository={registry}/{name}",
        name = hocfile.name
    ))?;

    shell.exit()?;
}

fn report(application_name: &str) {
    info!("{application_name} has been successfully deployed");
}

#[derive(Deserialize)]
struct Hocfile {
    name: String,
    version: String,
}

#[derive(Serialize)]
struct ChartContext<'a> {
    name: &'a str,
    app_version: &'a str,
}
