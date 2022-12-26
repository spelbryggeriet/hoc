use anyhow::Error;

use crate::{prelude::*, process};

#[throws(Error)]
pub fn run() {
    process::global_settings().container_mode();

    process!("cat /hoc/files/admin/ssh/pub").run()?;
}
