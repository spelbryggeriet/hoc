use std::fs::File;

use anyhow::Error;
use serde::Deserialize;

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
}

fn report(application_name: &str) {
    info!("{application_name} has been successfully deployed");
}

#[derive(Deserialize)]
struct Hocfile {
    name: String,
    version: String,
}
