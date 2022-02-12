use std::sync::{Arc, Mutex};

use console::Style;

use crate::{context::PrintContext, log::LogType, prefix::PrefixPrefs};

pub struct Status {
    print_context: Arc<Mutex<PrintContext>>,
    custom_label: Option<String>,
}

impl Status {
    pub(super) fn new(print_context: Arc<Mutex<PrintContext>>) -> Self {
        Status {
            print_context,
            custom_label: None,
        }
    }

    pub fn with_label(mut self, label: impl ToString) -> Self {
        self.custom_label.replace(label.to_string());
        self
    }
}
impl Drop for Status {
    fn drop(&mut self) {
        let mut print_context = self.print_context.lock().unwrap();

        let level = print_context.status_level();

        let mut line = String::new();
        if !print_context.failure {
            if let Some(label) = &self.custom_label {
                line += &label;
            } else {
                line += &Style::new().green().apply_to("success").to_string();
            }
        } else {
            line += &Style::new().red().apply_to("failure").to_string();
            if level == 1 {
                print_context.failure = false;
            }
        }

        print_context.decorated_println(
            line,
            None,
            LogType::StatusEnd,
            PrefixPrefs::with_connector("╙─").flag("─"),
            PrefixPrefs::with_connector("  ").flag(" "),
        );
        print_context.decrement_status();
    }
}
