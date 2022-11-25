use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
};

use console::Style;
use dialoguer::Password;

use crate::{context::PrintContext, info, prefix::PrefixPrefs};

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

    pub fn get(self) -> String {
        let mut prompt = self
            .print_context
            .lock()
            .unwrap()
            .create_line_prefix(PrefixPrefs::in_status().flag(">"));
        prompt += self.message.as_ref();
        prompt = Style::new().cyan().apply_to(prompt).to_string();
        let verify_prompt = prompt.clone() + " (verify)";

        let password = loop {
            let print_context = self.print_context.lock().unwrap();

            let password = Password::new()
                .with_prompt(&prompt)
                .interact_on(&print_context.stdout)
                .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

            if !self.verify {
                break password;
            }

            let password_verify = Password::new()
                .with_prompt(&verify_prompt)
                .interact_on(&print_context.stdout)
                .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

            if password == password_verify {
                break password;
            }

            drop(print_context);
            info!("The passwords don't match.");
        };

        password
    }
}
