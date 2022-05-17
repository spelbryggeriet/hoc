use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
};

use console::Style;
use dialoguer::Password;
use thiserror::Error;

use crate::{
    context::PrintContext,
    log::{LogErr, LogType},
    prefix::PrefixPrefs,
    Never,
};

#[derive(Debug, Error)]
pub enum Error {
    #[error("passwords does not match")]
    MismatchedPasswords,
}

impl From<Error> for crate::Error {
    fn from(err: Error) -> Self {
        Result::<Never, _>::Err(err)
            .log_context("hidden input")
            .unwrap_err()
    }
}

#[must_use]
pub struct HiddenInput<'a> {
    print_context: Arc<Mutex<PrintContext>>,
    message: Cow<'a, str>,
    verify: bool,
}

impl<'input> HiddenInput<'input> {
    pub(super) fn new(print_context: Arc<Mutex<PrintContext>>, message: Cow<'input, str>) -> Self {
        Self {
            print_context,
            message,
            verify: false,
        }
    }

    pub fn verify(mut self) -> Self {
        self.verify = true;
        self
    }

    pub fn get(self) -> Result<String, Error> {
        let mut print_context = self.print_context.lock().unwrap();

        print_context.print_spacing_if_needed(LogType::Input);

        let mut prompt = print_context.create_line_prefix(PrefixPrefs::in_status().flag(">"));
        prompt += self.message.as_ref();

        let cyan = Style::new().cyan();
        let password = Password::new()
            .with_prompt(cyan.apply_to(&prompt).to_string())
            .interact_on(&print_context.stdout)
            .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));
        if self.verify {
            prompt += " (verify)";
            let password_verify = Password::new()
                .with_prompt(cyan.apply_to(prompt).to_string())
                .interact_on(&print_context.stdout)
                .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

            if password != password_verify {
                print_context.failure = true;
                return Err(Error::MismatchedPasswords);
            }
        }

        Ok(password)
    }
}