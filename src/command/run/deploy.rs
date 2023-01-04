use std::time::Duration;
use std::{fmt::Write, fs::File};

use anyhow::Error;
use serde::Deserialize;
use serde::Serialize;
use tinytemplate::TinyTemplate;

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
    shell.run(process!(
        "helm upgrade {name} /helm/hoc-service/ --install --atomic --timeout {timeout} \
            --set image.repository={registry}/{image_name} \
            --set ingress.host={host} \
            --set service.port={port}",
        host = hocfile.service.domain,
        image_name = hocfile.image.name,
        name = hocfile.meta.name,
        port = hocfile.service.internal_port,
    ))?;

    shell.exit()?;
}

#[throws(Error)]
fn wait_on_pods(hocfile: &Hocfile) {
    progress!("Waiting on pods to be ready");

    const JSON_PATH: &str = "'\
        {range .items[*]}\
            {.status.phase}{\"=\"}\
            {range .status.containerStatuses[*]}\
                {.ready}{\"-\"}\
            {end}{\",\"}\
        {end}'";

    for _ in 0..30 {
        let output = process!(
            "kubectl get pods \
                -l=app.kubernetes.io/managed-by!=Helm,\
                   app.kubernetes.io/instance={name} \
                -o=jsonpath={JSON_PATH}",
            name = hocfile.meta.name,
        )
        .run()?;

        let all_ready = output.stdout.split_terminator(',').all(|pod| {
            pod.split_once('=').map_or(false, |(phase, statuses)| {
                phase == "Running"
                    && statuses
                        .split_terminator('-')
                        .all(|status| status == "true")
            })
        });

        if all_ready {
            break;
        };

        spin_sleep::sleep(Duration::from_secs(10));
    }
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
