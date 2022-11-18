use crate::{prelude::*, prompt};

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
