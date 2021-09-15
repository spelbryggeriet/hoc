mod styling;
mod wrapping;

use std::sync::{Arc, Mutex, Weak};
use std::{fmt, io::Write};

use console::{Style, Term};
use dialoguer::{theme::Theme, Confirm, Input, Password, Select};
use lazy_static::lazy_static;

pub use styling::Styling;
pub use wrapping::Wrapping;

const INFO_FLAG: &str = "~";
const ERROR_FLAG: &str = "⚠︎";

lazy_static! {
    pub static ref LOG: Log = Log::new();
}

#[macro_export]
macro_rules! info {
    ($fmt:expr) => {
        $crate::LOG.info($fmt)
    };

    ($($fmt:tt)*) => {
        info!(format!($($fmt)*))
    };
}

#[macro_export]
macro_rules! status {
    (($($fmt:tt)*) $($rest:tt)*) =>  {
        status!(format!($($fmt)*) $($rest)*)
    };

    ($fmt:expr => $code:expr) => {{
        let __status = $crate::LOG.status($fmt);
        $code
    }};
}

#[macro_export]
macro_rules! warning {
    ($fmt:expr) => {
        $crate::LOG.warning($fmt)
    };

    ($($fmt:tt)*) => {
        info!(format!($($fmt)*))
    };
}

#[macro_export]
macro_rules! error {
    ($fmt:expr) => {
        $crate::LOG.error($fmt)
    };

    ($($fmt:tt)*) => {
        info!(format!($($fmt)*))
    };
}

pub struct Log {
    print_context: Arc<Mutex<PrintContext>>,
}

struct PrintContext {
    stdout: Term,
    statuses: Vec<Weak<Status>>,
    failure: bool,
    last_log_type: Option<LogType>,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum LogType {
    /// The start part of a nested log, such as status.
    NestedStart,

    /// The end part of a nested log, such as status.
    NestedEnd,

    /// All other logs ar flat.
    Flat,
}

impl PrintContext {
    fn status_level(&self) -> usize {
        self.statuses.len()
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

    fn print_spacing_if_needed(&mut self, current_log_type: LogType) {
        let level = self.status_level();
        if (current_log_type == LogType::Flat && level == 0
            || current_log_type == LogType::NestedStart && level == 1)
            && self.last_log_type.is_some()
            && self.last_log_type != Some(current_log_type)
        {
            self.println("");
        }
    }

    fn decorated_println(
        &mut self,
        text: impl AsRef<str>,
        log_type: LogType,
        first_line_prefix_prefs: PrefixPrefs,
        line_prefix_prefs: PrefixPrefs,
    ) {
        self.print_spacing_if_needed(log_type);

        let prefix = self.create_line_prefix(first_line_prefix_prefs);
        let prefix_len = prefix.char_count_without_styling();

        let text_len = text.as_ref().chars().count();
        let text_max_width = self
            .stdout
            .size_checked()
            .and_then(|s| (s.1 as usize).checked_sub(prefix_len))
            .filter(|l| *l > 0)
            .unwrap_or(text_len);
        let normalized_text = text.as_ref().normalize_styling();
        let mut text_chunks = normalized_text.wrapped_lines(text_max_width);

        let first_line = prefix + &text_chunks.next().unwrap_or_default();
        self.println(first_line);

        for chunk in text_chunks {
            let mut line = self.create_line_prefix(line_prefix_prefs);
            line += &chunk;
            self.println(line);
        }

        self.last_log_type.replace(log_type);
    }

    fn create_line_prefix(&self, prefs: PrefixPrefs) -> String {
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

pub struct Stream<'a> {
    log: &'a Log,
    line: Mutex<String>,
}

pub struct Status {
    print_context: Arc<Mutex<PrintContext>>,
    tracking: bool,
}

#[derive(Copy, Clone)]
struct PrefixPrefs<'a> {
    connector: &'a str,
    flag: &'a str,
    label: &'a str,
}

impl Log {
    pub fn new() -> Self {
        Self {
            print_context: Arc::new(Mutex::new(PrintContext {
                stdout: Term::buffered_stdout(),
                statuses: Vec::new(),
                failure: false,
                last_log_type: None,
            })),
        }
    }

    pub fn stream(&self) -> Stream {
        Stream {
            log: self,
            line: Mutex::new(String::new()),
        }
    }

    pub fn status(&self, message: impl AsRef<str>) -> Arc<Status> {
        Status::register(message, &self.print_context, true)
    }

    pub fn status_no_track(&self, message: impl AsRef<str>) -> Arc<Status> {
        Status::register(message, &self.print_context, false)
    }

    pub fn info(&self, message: impl AsRef<str>) {
        let mut print_context = self.print_context.lock().unwrap();

        print_context.decorated_println(
            message,
            LogType::Flat,
            PrefixPrefs::in_status().flag(INFO_FLAG),
            PrefixPrefs::in_status_overflow(),
        );
    }

    pub fn labelled_info(&self, label: impl AsRef<str>, message: impl AsRef<str>) {
        let label_len = label.as_ref().chars().count();
        let label_trimmed = label.as_ref().trim_end().to_string();
        let label_trimmed_len = label_trimmed.chars().count();

        let mut label = label_trimmed;
        label += ":";
        label += &" ".repeat(label_len - label_trimmed_len);

        let mut print_context = self.print_context.lock().unwrap();

        print_context.decorated_println(
            message,
            LogType::Flat,
            PrefixPrefs::in_status().flag(INFO_FLAG).label(&label),
            PrefixPrefs::in_status_overflow().label(&" ".repeat(1 + label_len)),
        );
    }

    pub fn warning(&self, message: impl AsRef<str>) {
        let mut print_context = self.print_context.lock().unwrap();

        let yellow = Style::new().yellow();

        print_context.decorated_println(
            yellow.apply_to(message.as_ref()).to_string(),
            LogType::Flat,
            PrefixPrefs::in_status().flag(&yellow.apply_to(ERROR_FLAG).to_string()),
            PrefixPrefs::in_status_overflow(),
        );
    }

    pub fn error(&self, message: impl AsRef<str>) {
        let mut print_context = self.print_context.lock().unwrap();

        let red = Style::new().red();
        print_context.failure = true;

        print_context.decorated_println(
            red.apply_to(message.as_ref()).to_string(),
            LogType::Flat,
            PrefixPrefs::in_status().flag(&red.apply_to(ERROR_FLAG).to_string()),
            PrefixPrefs::in_status_overflow(),
        );
    }

    pub fn prompt(&self, message: impl AsRef<str>) -> bool {
        let mut print_context = self.print_context.lock().unwrap();

        let cyan = Style::new().cyan();

        let mut prompt = print_context.create_line_prefix(PrefixPrefs::in_status().flag("?"));
        prompt += message.as_ref();

        let want_continue = Confirm::new()
            .with_prompt(cyan.apply_to(prompt).to_string())
            .default(false)
            .interact_on(&print_context.stdout)
            .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

        print_context.last_log_type.replace(LogType::Flat);

        want_continue
    }

    pub fn input(&self, message: impl AsRef<str>) -> String {
        let mut print_context = self.print_context.lock().unwrap();

        let cyan = Style::new().cyan();

        let mut prompt = print_context.create_line_prefix(PrefixPrefs::in_status().flag("?"));
        prompt += message.as_ref();

        let input = Input::new()
            .with_prompt(cyan.apply_to(prompt).to_string())
            .interact_on(&print_context.stdout)
            .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

        print_context.last_log_type.replace(LogType::Flat);

        input
    }

    pub fn hidden_input(&self, message: impl AsRef<str>) -> String {
        let mut print_context = self.print_context.lock().unwrap();

        let cyan = Style::new().cyan();

        let mut prompt = print_context.create_line_prefix(PrefixPrefs::in_status().flag("?"));
        prompt += message.as_ref();

        let password = Password::new()
            .with_prompt(cyan.apply_to(prompt).to_string())
            .interact_on(&print_context.stdout)
            .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

        print_context.last_log_type.replace(LogType::Flat);

        password
    }

    pub fn choose(
        &self,
        message: impl AsRef<str>,
        items: impl IntoIterator<Item = impl ToString>,
        default_index: usize,
    ) -> usize {
        let mut print_context = self.print_context.lock().unwrap();

        let cyan = Style::new().cyan();
        let items: Vec<_> = items.into_iter().collect();

        let mut prompt = print_context.create_line_prefix(PrefixPrefs::in_status().flag("#"));
        prompt += message.as_ref();

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

        let index = Select::with_theme(&ChooseTheme {
            print_context: &print_context,
        })
        .with_prompt(cyan.apply_to(prompt).to_string())
        .items(&items)
        .default(default_index)
        .interact_on_opt(&print_context.stdout)
        .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

        print_context.last_log_type.replace(LogType::Flat);

        if let Some(index) = index {
            index
        } else {
            // anyhow::bail!("User cancelled operation");
            panic!("User cancelled operation");
        }
    }
}

impl Stream<'_> {
    pub fn process(&self, stream: impl AsRef<str>) {
        let mut chunks = stream.as_ref().split('\n');

        // Always append the first chunk unconditionally.
        *self.line.lock().unwrap() += chunks.next().unwrap();

        for chunk in chunks {
            let mut line = self.line.lock().unwrap();

            let active_code = line.active_ansi_escape_code().map(ToString::to_string);
            if active_code.is_some() {
                *line += styling::CLEAR_STYLE;
            }

            self.log.info(&*line);
            line.clear();

            if let Some(active_code) = active_code {
                *line += &active_code;
            }
            *line += chunk;
        }
    }
}

impl Drop for Stream<'_> {
    fn drop(&mut self) {
        let line = self.line.lock().unwrap();
        if line.len() > 0 {
            self.log.info(&*line);
        }
    }
}

impl Status {
    fn register(
        message: impl AsRef<str>,
        print_context: &Arc<Mutex<PrintContext>>,
        tracking: bool,
    ) -> Arc<Self> {
        let mut print_context_unlocked = print_context.lock().unwrap();

        let status = Arc::new(Status {
            print_context: Arc::clone(&print_context),
            tracking,
        });

        print_context_unlocked
            .statuses
            .push(Arc::downgrade(&status));

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

        print_context.statuses.pop();
    }
}

impl<'a> PrefixPrefs<'a> {
    fn with_connector(connector: &'a str) -> Self {
        Self {
            connector,
            flag: " ",
            label: "",
        }
    }

    fn in_status() -> Self {
        Self::with_connector("╟╴")
    }

    fn in_status_overflow() -> Self {
        Self::with_connector("║ ")
    }

    fn flag(mut self, flag: &'a str) -> Self {
        self.flag = flag;
        self
    }

    fn label(mut self, label: &'a str) -> Self {
        self.label = label;
        self
    }
}
