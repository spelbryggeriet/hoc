use std::sync::{Arc, Mutex};

use console::Style;

use crate::{context::PrintContext, log::LogType, prefix::PrefixPrefs};

pub struct Status {
    print_context: Arc<Mutex<PrintContext>>,
    tracking: bool,
}

impl Status {
    pub fn register(
        message: impl AsRef<str>,
        print_context: &Arc<Mutex<PrintContext>>,
        tracking: bool,
    ) -> Arc<Self> {
        let mut print_context_unlocked = print_context.lock().unwrap();

        let status = Arc::new(Status {
            print_context: Arc::clone(&print_context),
            tracking,
        });

        print_context_unlocked.push_status(Arc::downgrade(&status));

        print_context_unlocked.decorated_println(
            message,
            LogType::NestedStart,
            PrefixPrefs::with_connector("╓╴").flag("*"),
            PrefixPrefs::in_status_overflow(),
        );

        status
    }
}

impl Drop for Status {
    fn drop(&mut self) {
        let mut print_context = self.print_context.lock().unwrap();

        let level = print_context.status_level();

        let mut line = String::new();
        if self.tracking {
            if !print_context.failure {
                if level == 1 {
                    line += &Style::new().green().apply_to("SUCCESS").to_string();
                } else {
                    line += "DONE";
                }
            } else {
                line += &Style::new().red().apply_to("FAILURE").to_string();
            }
        } else {
            line += "DONE";
        };

        print_context.decorated_println(
            line,
            LogType::NestedEnd,
            PrefixPrefs::with_connector("╙─").flag("─"),
            PrefixPrefs::with_connector("  ").flag(" "),
        );

        print_context.pop_status();
    }
}
