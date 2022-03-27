use std::{
    sync::{Arc, Mutex},
    time::SystemTime,
};

use console::Style;

use crate::{context::PrintContext, log::LogType, prefix::PrefixPrefs};

pub struct Status {
    print_context: Arc<Mutex<PrintContext>>,
    custom_label: Option<String>,
    start_time: SystemTime,
}

impl Status {
    pub(super) fn new(print_context: Arc<Mutex<PrintContext>>) -> Self {
        Status {
            print_context,
            custom_label: None,
            start_time: SystemTime::now(),
        }
    }

    pub fn with_label(mut self, label: impl ToString) -> Self {
        self.custom_label.replace(label.to_string());
        self
    }
}
impl Drop for Status {
    fn drop(&mut self) {
        let blue = Style::new().blue();

        let time = self.start_time.elapsed().unwrap().as_secs_f32();
        let time_str = if time >= 60.0 {
            let mins = (time / 60.0).floor();
            let secs = (time % 60.0).floor();
            blue.apply_to(format!("{mins:.0}m{secs:.0}s"))
        } else if time >= 10.0 {
            let secs = time.floor();
            blue.apply_to(format!("{secs:.0}s"))
        } else {
            blue.apply_to(format!("{time:.2}s"))
        };

        let mut print_context = self.print_context.lock().unwrap();

        let level = print_context.status_level();

        let mut line = String::new();
        if !print_context.failure {
            if let Some(label) = &self.custom_label {
                line += &format!("({time_str}) {label}");
            } else {
                line += &format!(
                    "({time_str}) {}",
                    Style::new().green().apply_to("success").to_string()
                );
            }
        } else {
            line += &format!(
                "({time_str}) {}",
                Style::new().red().apply_to("failure").to_string()
            );
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
