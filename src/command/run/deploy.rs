use std::time::Duration;
use std::{fmt::Write, fs::File};

use anyhow::Error;
use serde::Deserialize;
use serde::Serialize;
use tinytemplate::TinyTemplate;

use crate::command::util::{self, Helm};
use crate::prelude::*;

#[throws(Error)]
pub fn run(timeout: String) {
    let file = open_hocfile()?;
    let hocfile = parse_hocfile(file)?;
    deploy_application(&hocfile, &timeout)?;
    wait_on_pods(&hocfile)?;
    test_deployment(&hocfile, &timeout)?;
    report(&hocfile.meta.name);
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
fn deploy_application(hocfile: &Hocfile, timeout: &str) {
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
        name: &hocfile.meta.name,
        app_version: &hocfile.meta.version,
    };

    let chart = tt.render("chart", &context)?;

    let registry: String = kv!("registry/prefix").get()?.convert()?;

    shell.run(process!("tee /helm/hoc-service/Chart.yaml" < ("{chart}")))?;
    shell.run(
        Helm.upgrade(&hocfile.meta.name, "/helm/hoc-service/")
            .timeout(timeout)
            .set(
                "image.repository",
                &format!("{registry}/{image_name}", image_name = hocfile.image.name),
            )
            .set("ingress.host", &hocfile.service.domain)
            .set("service.port", &hocfile.service.internal_port.to_string()),
    )?;

    shell.exit()?;
}

#[throws(Error)]
fn wait_on_pods(hocfile: &Hocfile) {
    progress!("Waiting on pods to be ready");

    util::k8s_wait_on_pods(&hocfile.meta.name)?;
}

#[throws(Error)]
fn test_deployment(hocfile: &Hocfile, timeout: &str) {
    progress!("Testing deployment");

    process!(
        "helm test {name} --timeout {timeout}",
        name = hocfile.meta.name,
    )
    .run()?;
}

fn report(application_name: &str) {
    info!("{application_name} has been successfully deployed");
}

#[derive(Deserialize)]
struct Hocfile {
    meta: Meta,
    image: Image,
    service: Service,
}

#[derive(Deserialize)]
struct Meta {
    name: String,
    version: String,
}

#[derive(Deserialize)]
struct Image {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Service {
    domain: String,
    internal_port: u16,
}

#[derive(Serialize)]
struct ChartContext<'a> {
    name: &'a str,
    app_version: &'a str,
}
