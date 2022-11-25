use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
};

use console::Style;
use dialoguer::Confirm;
use thiserror::Error;

use crate::{
    context::PrintContext,
    log::{LogErr, LogType},
    prefix::PrefixPrefs,
    Never,
};

#[derive(Debug, Error)]
pub enum Error {
    #[error("user aborted")]
    Aborted,
}

impl From<Error> for crate::Error {
    fn from(err: Error) -> Self {
        Result::<Never, _>::Err(err)
            .log_context("prompt")
            .unwrap_err()
    }
}

#[must_use]
pub struct Prompt<'a> {
    print_context: Arc<Mutex<PrintContext>>,
    message: Cow<'a, str>,
}

impl<'prompt> Prompt<'prompt> {
    pub(super) fn new(print_context: Arc<Mutex<PrintContext>>, message: Cow<'prompt, str>) -> Self {
        Self {
            print_context,
            message,
        }
    }
}

impl<'a> Prompt<'a> {
    pub fn get(self) -> Result<(), Error> {
        let mut print_context = self.print_context.lock().unwrap();
        print_context.print_spacing_if_needed(LogType::Prompt);

        let mut prompt = print_context.create_line_prefix(PrefixPrefs::in_status().flag("?"));
        prompt += self.message.as_ref();

        let cyan = Style::new().cyan();
        let want_continue = Confirm::new()
            .with_prompt(cyan.apply_to(prompt).to_string())
            .default(false)
            .interact_on(&print_context.stdout)
            .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

        if want_continue {
            Ok(())
        } else {
            print_context.failure = true;
            Err(Error::Aborted)
        }
    }
}
