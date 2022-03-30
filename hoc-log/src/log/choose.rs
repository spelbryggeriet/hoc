use std::{
    borrow::Cow,
    fmt,
    sync::{Arc, Mutex},
};

use console::Style;
use dialoguer::{theme::Theme, Select};
use thiserror::Error;

use crate::{context::PrintContext, log::LogType, prefix::PrefixPrefs};

#[derive(Debug, Error)]
pub enum Error {
    #[error("empty items list")]
    EmptyItemsList,

    #[error("user aborted")]
    Aborted,
}

pub struct Choose<'a, T> {
    print_context: Arc<Mutex<PrintContext>>,
    message: Cow<'a, str>,
    items: &'a [T],
    default_index: usize,
}

impl<'choose, T> Choose<'choose, T> {
    pub(super) fn new(print_context: Arc<Mutex<PrintContext>>, message: Cow<'choose, str>) -> Self {
        Self {
            print_context,
            message,
            items: &[],
            default_index: 0,
        }
    }

    pub fn items<I: AsRef<[T]>>(mut self, items: &'choose I) -> Self {
        self.items = items.as_ref();
        self
    }

    pub fn default_index(mut self, index: usize) -> Self {
        self.default_index = index;
        self
    }
}

impl<'a, T: ToString> Choose<'a, T> {
    pub fn get(self) -> Result<usize, Error> {
        let mut print_context = self.print_context.lock().unwrap();

        if self.items.len() == 0 {
            print_context.failure = true;
            return Err(Error::EmptyItemsList);
        }

        print_context.print_spacing_if_needed(LogType::Choose);

        let mut prompt = print_context.create_line_prefix(PrefixPrefs::in_status().flag("#"));
        prompt += self.message.as_ref();

        struct ChooseTheme<'a> {
            print_context: &'a PrintContext,
        }

        impl Theme for ChooseTheme<'_> {
            fn format_select_prompt_item(
                &self,
                f: &mut dyn fmt::Write,
                text: &str,
                active: bool,
            ) -> fmt::Result {
                let prefix = self.print_context.create_line_prefix(
                    PrefixPrefs::in_status_overflow().flag(if active { ">" } else { " " }),
                );
                write!(f, "{}{}", prefix, text)
            }
        }

        let cyan = Style::new().cyan();
        let index = Select::with_theme(&ChooseTheme {
            print_context: &print_context,
        })
        .with_prompt(cyan.apply_to(prompt).to_string())
        .items(&self.items)
        .default(self.default_index)
        .interact_on_opt(&print_context.stdout)
        .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

        if let Some(index) = index {
            Ok(index)
        } else {
            print_context.failure = true;
            Err(Error::Aborted)
        }
    }
}
