use std::{io::SeekFrom, path::Path};

use tokio::{fs::File, io::AsyncSeekExt};

use crate::{
    context::{self, key::Key, CachedFileFn},
    prelude::*,
    prompt,
};

#[throws(prompt::Error)]
pub fn overwrite_prompt() -> bool {
    select!("How do you want to resolve the file path conflict?")
        .with_option("Skip", || false)
        .with_option("Overwrite", || true)
        .get()?
}

#[throws(prompt::Error)]
pub fn retry_prompt() -> bool {
    select!("How do you want to resolve the error?")
        .with_option("Retry", || true)
        .get()?
}

#[throws(context::Error)]
pub async fn cache_loop<C>(key: &Key, file: &mut File, path: &Path, on_cache: C)
where
    C: for<'a> CachedFileFn<'a>,
{
    let caching_progress = progress_with_handle!("Caching file for key {:}", key);

    let mut retrying = false;
    loop {
        if let Err(err) = on_cache(file, &path, retrying).await {
            let custom_err = err.into();
            error!("{custom_err}");
            retrying = context::util::retry_prompt()?;
        } else {
            break;
        };

        if retrying {
            file.set_len(0).await?;
            file.seek(SeekFrom::Start(0)).await?;
        }
    }

    caching_progress.finish();
}
