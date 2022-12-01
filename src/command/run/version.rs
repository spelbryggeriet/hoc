use crossterm::style::Stylize;

use crate::prelude::*;

pub fn run() {
    info!(
        "{} is at version v{}",
        "hoc".yellow(),
        env!("CARGO_PKG_VERSION"),
    );
}
