use std::io::Write;

use console::{Style, Term};

use crate::{log::LogType, prefix::PrefixPrefs, styling::Styling, wrapping::Wrap};

pub struct PrintContext {
    pub failure: bool,
    pub stdout: Term,
    statuses: usize,
    last_log_type: Option<LogType>,
}

impl PrintContext {
    pub fn new() -> Self {
        PrintContext {
            failure: false,
            stdout: Term::buffered_stdout(),
            statuses: 0,
            last_log_type: None,
        }
    }

    pub fn status_level(&self) -> usize {
        self.statuses
    }

    pub fn increment_status(&mut self) {
        self.statuses += 1;
    }

    pub fn decrement_status(&mut self) {
        self.statuses -= 1;
    }

    pub fn decorated_println(
        &mut self,
        text: impl AsRef<str>,
        color: Option<Style>,
        log_type: LogType,
        first_line_prefix_prefs: PrefixPrefs,
        line_prefix_prefs: PrefixPrefs,
    ) {
        let mut prefix_prefs = Some(first_line_prefix_prefs);
        let mut get_prefix_prefs = || prefix_prefs.take().unwrap_or(line_prefix_prefs);

        for line in text.as_ref().lines() {
            self.print_spacing_if_needed(log_type);

            let prefix = self.create_line_prefix(get_prefix_prefs());
            let prefix_len = prefix.visible_char_indices().count();

            let text_len = line.visible_char_indices().count();
            let text_max_width = self
                .stdout
                .size_checked()
                .and_then(|s| (s.1 as usize).checked_sub(prefix_len))
                .filter(|l| *l > 0)
                .unwrap_or(text_len);
            let mut line_chunks = Some(line).into_iter().wrap(text_max_width);

            let first_line = if let Some(ref color) = color {
                prefix
                    + &color
                        .apply_to(line_chunks.next().unwrap_or_default())
                        .to_string()
            } else {
                prefix + &line_chunks.next().unwrap_or_default()
            };
            self.println(first_line);

            for mut chunk in line_chunks {
                chunk = if let Some(color) = &color {
                    self.create_line_prefix(get_prefix_prefs()) + &color.apply_to(chunk).to_string()
                } else {
                    self.create_line_prefix(get_prefix_prefs()) + &chunk
                };
                self.println(chunk);
            }
        }
    }

    pub fn create_line_prefix(&self, prefs: PrefixPrefs) -> String {
        let mut line_prefix = String::new();

        let level = self.status_level();

        if level > 0 {
            for outer_level in 1..level {
                line_prefix += &self
                    .get_status_level_color(outer_level)
                    .apply_to("│ ")
                    .to_string();
            }

            let status_level_color = self.get_status_level_color(self.status_level());
            line_prefix += &status_level_color.apply_to(prefs.connector).to_string();
            line_prefix += &status_level_color.apply_to(prefs.flag).to_string();
        } else {
            line_prefix += prefs.flag;
        }

        line_prefix += " ";

        if prefs.label.len() > 0 {
            line_prefix += prefs.label;
            line_prefix += " ";
        }

        line_prefix
    }

    pub fn print_spacing_if_needed(&mut self, current_log_type: LogType) {
        if self.last_log_type.is_some()
            && self.last_log_type != Some(current_log_type)
            && self.last_log_type != Some(LogType::StatusStart)
            && current_log_type != LogType::StatusEnd
        {
            self.println(self.create_line_prefix(PrefixPrefs::in_status_overflow()));
        }

        self.last_log_type.replace(current_log_type);
    }

    fn print(&mut self, msg: impl AsRef<str>, flush: bool) {
        self.stdout
            .write(msg.as_ref().as_bytes())
            .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));
        if flush {
            self.stdout
                .flush()
                .unwrap_or_else(|e| panic!("failed flushing to stdout: {}", e));
        }
    }

    fn println(&mut self, msg: impl AsRef<str>) {
        self.print(msg, false);
        self.print("\n", true);
    }

    fn get_status_level_color(&self, status_level: usize) -> Style {
        let style = Style::new();
        match status_level {
            0 | 1 => style.white(),
            2 => style.white().bright(),
            3 => style.cyan(),
            4 => style.cyan().bright(),
            5 => style.blue(),
            6 => style.blue().bright(),
            7 => style.magenta(),
            _ => style.magenta().bright(),
        }
    }
}
