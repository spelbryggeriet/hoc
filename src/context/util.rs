use std::io::{Seek, SeekFrom};

use crate::{
    context::{fs::ContextFile, key::Key, Error},
    prelude::*,
    util::Opt,
};

#[throws(Error)]
pub fn cache_loop<C>(key: &Key, file: &mut ContextFile, on_cache: C)
where
    C: Fn(&mut ContextFile, bool) -> Result<(), Error>,
{
    progress!("Caching file for key {key:?}");

    let mut retrying = false;
    loop {
        if let Err(err) = on_cache(file, retrying) {
            error!("{err}");
            select!("How do you want to resolve the error?")
                .with_option(Opt::Retry)
                .get()?;
            retrying = true;
        } else {
            break;
        };

        if retrying {
            file.file.set_len(0)?;
            file.file.seek(SeekFrom::Start(0))?;
        }
    }
}
