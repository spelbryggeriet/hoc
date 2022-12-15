use std::{
    fs::File,
    io::{Seek, SeekFrom},
    path::Path,
};

use crate::{
    context::{self, key::Key},
    prelude::*,
    util::Opt,
};

#[throws(context::Error)]
pub fn cache_loop<C>(key: &Key, file: &mut File, path: &Path, on_cache: C)
where
    C: Fn(&mut File, &Path, bool) -> Result<(), context::Error>,
{
    progress!("Caching file for key {key}");

    let mut retrying = false;
    loop {
        if let Err(err) = on_cache(file, path, retrying) {
            error!("{err}");
            select!("How do you want to resolve the error?")
                .with_option(Opt::Retry)
                .get()?;
            retrying = true;
        } else {
            break;
        };

        if retrying {
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
        }
    }
}
