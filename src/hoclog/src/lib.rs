mod styling;
mod wrapping;

use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::{fmt, io::Write};

use console::{Style, Term, TermTarget};
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
    ($fmt:expr => $code:expr) => {{
        let __status = $crate::LOG.status($fmt);
        $code
    }};

    (($($fmt:tt)*) $($rest:tt)*) =>  {
        status!(format!($($fmt)*) $($rest)*)
    };
}

fn get_term_label(target: TermTarget) -> &'static str {
    match target {
        TermTarget::Stdout => "stdout",
        TermTarget::Stderr => "stderr",
    }
}

pub struct Log {
    print_context: Arc<Mutex<PrintContext>>,
}

struct PrintContext {
    stdout: Term,
    statuses: Vec<Weak<Status>>,
    failure: bool,
    spacing_needed: bool,
}

impl PrintContext {
    fn calculate_status_level(&self) -> Option<usize> {
        Some(self.statuses.len()).filter(|i| *i > 0).map(|i| i - 1)
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
                spacing_needed: false,
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

        let level = print_context.calculate_status_level();
        Log::println_wrapped_text(
            &mut print_context.stdout,
            message,
            level,
            PrefixPrefs::in_status().flag(INFO_FLAG),
            PrefixPrefs::in_status_overflow(),
        );

        print_context.spacing_needed = true;
    }

    pub fn labelled_info(&self, label: impl AsRef<str>, message: impl AsRef<str>) {
        let mut print_context = self.print_context.lock().unwrap();

        let level = print_context.calculate_status_level();

        let label_len = label.as_ref().chars().count();
        let label_trimmed = label.as_ref().trim_end().to_string();
        let label_trimmed_len = label_trimmed.chars().count();

        let mut label = label_trimmed;
        label += ":";
        label += &" ".repeat(label_len - label_trimmed_len);

        Log::println_wrapped_text(
            &mut print_context.stdout,
            message,
            level,
            PrefixPrefs::in_status().flag(INFO_FLAG).label(&label),
            PrefixPrefs::in_status_overflow().label(&" ".repeat(1 + label_len)),
        );

        print_context.spacing_needed = true;
    }

    pub fn warning(&self, message: impl AsRef<str>) {
        let mut print_context = self.print_context.lock().unwrap();

        let yellow = Style::new().yellow();
        let level = print_context.calculate_status_level();

        Log::println_wrapped_text(
            &mut print_context.stdout,
            yellow.apply_to(message.as_ref()).to_string(),
            level,
            PrefixPrefs::in_status().flag(&yellow.apply_to(ERROR_FLAG).to_string()),
            PrefixPrefs::in_status_overflow(),
        );

        print_context.spacing_needed = true;
    }

    pub fn error(&self, message: impl AsRef<str>) {
        let mut print_context = self.print_context.lock().unwrap();

        let red = Style::new().red();
        print_context.failure = true;
        let level = print_context.calculate_status_level();

        Log::println_wrapped_text(
            &mut print_context.stdout,
            red.apply_to(message.as_ref()).to_string(),
            level,
            PrefixPrefs::in_status().flag(&red.apply_to(ERROR_FLAG).to_string()),
            PrefixPrefs::in_status_overflow(),
        );

        print_context.spacing_needed = true;
    }

    pub fn prompt(&self, message: impl AsRef<str>) -> bool {
        let mut print_context = self.print_context.lock().unwrap();

        let cyan = Style::new().cyan();
        let level = print_context.calculate_status_level();

        let mut prompt = Log::create_line_prefix(level, PrefixPrefs::in_status().flag("?"));
        prompt += message.as_ref();

        let want_continue = Confirm::new()
            .with_prompt(cyan.apply_to(prompt).to_string())
            .default(false)
            .interact_on(&print_context.stdout)
            .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

        print_context.spacing_needed = true;

        want_continue
    }

    pub fn input(&self, message: impl AsRef<str>) -> String {
        let mut print_context = self.print_context.lock().unwrap();

        let cyan = Style::new().cyan();
        let level = print_context.calculate_status_level();

        let mut prompt = Log::create_line_prefix(level, PrefixPrefs::in_status().flag("?"));
        prompt += message.as_ref();

        let input = Input::new()
            .with_prompt(cyan.apply_to(prompt).to_string())
            .interact_on(&print_context.stdout)
            .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

        print_context.spacing_needed = true;

        input
    }

    pub fn hidden_input(&self, message: impl AsRef<str>) -> String {
        let mut print_context = self.print_context.lock().unwrap();

        let cyan = Style::new().cyan();
        let level = print_context.calculate_status_level();

        let mut prompt = Log::create_line_prefix(level, PrefixPrefs::in_status().flag("?"));
        prompt += message.as_ref();

        let password = Password::new()
            .with_prompt(cyan.apply_to(prompt).to_string())
            .interact_on(&print_context.stdout)
            .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));
        print_context.spacing_needed = true;

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
        let level = print_context.calculate_status_level();

        let mut prompt = Log::create_line_prefix(level, PrefixPrefs::in_status().flag("#"));
        prompt += message.as_ref();

        struct ChooseTheme {
            level: Option<usize>,
        }

        impl Theme for ChooseTheme {
            fn format_select_prompt_item(
                &self,
                f: &mut dyn fmt::Write,
                text: &str,
                active: bool,
            ) -> fmt::Result {
                let prefix = Log::create_line_prefix(
                    self.level,
                    PrefixPrefs::in_status_overflow().flag(if active { ">" } else { " " }),
                );
                write!(f, "{}{}", prefix, text)
            }
        }

        let index = Select::with_theme(&ChooseTheme { level })
            .with_prompt(cyan.apply_to(prompt).to_string())
            .items(&items)
            .default(default_index)
            .interact_on_opt(&print_context.stdout)
            .unwrap_or_else(|e| panic!("failed printing to stdout: {}", e));

        print_context.spacing_needed = true;

        if let Some(index) = index {
            index
        } else {
            // anyhow::bail!("User cancelled operation");
            panic!("User cancelled operation");
        }
    }

    fn print(out: &mut Term, msg: impl AsRef<str>) {
        out.write(msg.as_ref().as_bytes()).unwrap_or_else(|e| {
            panic!("failed printing to {}: {}", get_term_label(out.target()), e)
        });
        out.flush().unwrap_or_else(|e| {
            panic!("failed flushing to {}: {}", get_term_label(out.target()), e)
        });
    }

    fn println(out: &mut Term, msg: impl AsRef<str>) {
        Log::print(out, msg);
        Log::print(out, "\n");
    }

    fn println_wrapped_text(
        out: &mut Term,
        text: impl AsRef<str>,
        status_level: Option<usize>,
        first_line_prefix_prefs: PrefixPrefs,
        line_prefix_prefs: PrefixPrefs,
    ) {
        let prefix = Log::create_line_prefix(status_level, first_line_prefix_prefs);
        let prefix_len = prefix.char_count_without_styling();

        let text_len = text.as_ref().chars().count();
        let text_max_width = out
            .size_checked()
            .and_then(|s| (s.1 as usize).checked_sub(prefix_len))
            .filter(|l| *l > 0)
            .unwrap_or(text_len);
        let normalized_text = text.as_ref().normalize_styling();
        let mut text_chunks = normalized_text.wrapped_lines(text_max_width);

        let first_line = prefix + &text_chunks.next().unwrap_or_default();
        Log::println(out, first_line);

        for chunk in text_chunks {
            let mut line = Log::create_line_prefix(status_level, line_prefix_prefs);
            line += &chunk;
            Log::println(out, line);
        }
    }

    fn get_status_level_color(level: usize) -> Style {
        let style = Style::new();
        match level {
            0 => style.white(),
            1 => style.white().bright(),
            2 => style.cyan(),
            3 => style.cyan().bright(),
            4 => style.blue(),
            5 => style.blue().bright(),
            6 => style.magenta(),
            _ => style.magenta().bright(),
        }
    }

    fn create_line_prefix(status_level: Option<usize>, prefs: PrefixPrefs) -> String {
        let mut line_prefix = String::new();

        if let Some(level) = status_level {
            for outer_level in 0..level {
                line_prefix += &Log::get_status_level_color(outer_level)
                    .apply_to("│ ")
                    .to_string();
            }

            let status_level_color = Log::get_status_level_color(level);
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
        let print_context_clone = Arc::clone(print_context);
        let mut print_context = print_context.lock().unwrap();

        let level = print_context
            .calculate_status_level()
            .map(|l| Some(l + 1))
            .unwrap_or(Some(0));

        if level == Some(0) && print_context.spacing_needed {
            Log::println(&mut print_context.stdout, "");
            print_context.spacing_needed = false;
        }

        Log::println_wrapped_text(
            &mut print_context.stdout,
            message,
            level,
            PrefixPrefs::with_connector("╓╴").flag("*"),
            PrefixPrefs::in_status_overflow(),
        );

        let status = Arc::new(Status {
            print_context: print_context_clone,
            tracking,
        });

        print_context.statuses.push(Arc::downgrade(&status));

        status
    }
}

impl Drop for Status {
    fn drop(&mut self) {
        let mut print_context = self.print_context.lock().unwrap();

        let level = print_context.calculate_status_level();

        let mut line = Log::create_line_prefix(level, PrefixPrefs::with_connector("╙─").flag("─"));
        if self.tracking {
            if !print_context.failure {
                if level == Some(0) {
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

        Log::println(&mut print_context.stdout, line);

        if level == Some(0) {
            Log::println(&mut print_context.stdout, "");
            print_context.spacing_needed = false;
        }

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
