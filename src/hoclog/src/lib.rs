mod styling;
mod wrapping;

use console::{Style, Term, TermTarget};
use dialoguer::{theme::Theme, Confirm, Input, Password, Select};
use std::sync::{Arc, Mutex, Weak};
use std::{fmt, io::Write};

pub use styling::Styling;
pub use wrapping::Wrapping;

const INFO_FLAG: &str = "~";
const ERROR_FLAG: &str = "⚠︎";

fn get_term_label(target: TermTarget) -> &'static str {
    match target {
        TermTarget::Stdout => "stdout",
        TermTarget::Stderr => "stderr",
    }
}

type Statuses = Arc<Mutex<Vec<Weak<Status>>>>;

pub struct Log {
    stdout: Arc<Mutex<Term>>,
    statuses: Statuses,
    failure: Arc<Mutex<bool>>,
    spacing_printed: Arc<Mutex<bool>>,
}

pub struct Stream<'a> {
    log: &'a Log,
    line: Mutex<String>,
}

pub struct Status {
    stdout: Arc<Mutex<Term>>,
    statuses: Statuses,
    failure: Arc<Mutex<bool>>,
    spacing_printed: Arc<Mutex<bool>>,
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
            stdout: Arc::new(Mutex::new(Term::buffered_stdout())),
            statuses: Arc::new(Mutex::new(vec![])),
            failure: Arc::new(Mutex::new(false)),
            spacing_printed: Arc::new(Mutex::new(false)),
        }
    }

    pub fn stream(&self) -> Stream {
        Stream {
            log: self,
            line: Mutex::new(String::new()),
        }
    }

    pub fn status(&self, message: impl AsRef<str>) -> Arc<Status> {
        let status = Arc::new(Status::new(
            Arc::clone(&self.stdout),
            message,
            Arc::clone(&self.statuses),
            Arc::clone(&self.failure),
            Arc::clone(&self.spacing_printed),
            true,
        ));

        self.statuses.lock().unwrap().push(Arc::downgrade(&status));
        *self.spacing_printed.lock().unwrap() = true;
        status
    }

    pub fn status_no_track(&self, message: impl AsRef<str>) -> Arc<Status> {
        let status = Arc::new(Status::new(
            Arc::clone(&self.stdout),
            message,
            Arc::clone(&self.statuses),
            Arc::clone(&self.failure),
            Arc::clone(&self.spacing_printed),
            false,
        ));

        self.statuses.lock().unwrap().push(Arc::downgrade(&status));
        *self.spacing_printed.lock().unwrap() = true;
        status
    }

    pub fn info(&self, message: impl AsRef<str>) {
        let level = Log::calculate_status_level(&self.statuses);
        Log::println_wrapped_text(
            &self.stdout,
            message,
            level,
            PrefixPrefs::in_status().flag(INFO_FLAG),
            PrefixPrefs::in_status_overflow(),
        );
        *self.spacing_printed.lock().unwrap() = false;
    }

    pub fn labelled_info(&self, label: impl AsRef<str>, message: impl AsRef<str>) {
        let level = Log::calculate_status_level(&self.statuses);

        let label_len = label.as_ref().chars().count();
        let label_trimmed = label.as_ref().trim_end().to_string();
        let label_trimmed_len = label_trimmed.chars().count();

        let mut label = label_trimmed;
        label += ":";
        label += &" ".repeat(label_len - label_trimmed_len);

        Log::println_wrapped_text(
            &self.stdout,
            message,
            level,
            PrefixPrefs::in_status().flag(INFO_FLAG).label(&label),
            PrefixPrefs::in_status_overflow().label(&" ".repeat(1 + label_len)),
        );
        *self.spacing_printed.lock().unwrap() = false;
    }

    pub fn warning(&self, message: impl AsRef<str>) {
        let yellow = Style::new().yellow();
        let level = Log::calculate_status_level(&self.statuses);

        Log::println_wrapped_text(
            &self.stdout,
            yellow.apply_to(message.as_ref()).to_string(),
            level,
            PrefixPrefs::in_status().flag(&yellow.apply_to(ERROR_FLAG).to_string()),
            PrefixPrefs::in_status_overflow(),
        );

        *self.spacing_printed.lock().unwrap() = false;
    }

    pub fn error(&self, message: impl AsRef<str>) {
        let red = Style::new().red();
        *self.failure.lock().unwrap() = true;
        let level = Log::calculate_status_level(&self.statuses);

        Log::println_wrapped_text(
            &self.stdout,
            red.apply_to(message.as_ref()).to_string(),
            level,
            PrefixPrefs::in_status().flag(&red.apply_to(ERROR_FLAG).to_string()),
            PrefixPrefs::in_status_overflow(),
        );

        *self.spacing_printed.lock().unwrap() = false;
    }

    pub fn prompt(&self, message: impl AsRef<str>) {
        let cyan = Style::new().cyan();
        let level = Log::calculate_status_level(&self.statuses);

        let mut prompt = Log::create_line_prefix(level, PrefixPrefs::in_status().flag("?"));
        prompt += message.as_ref();

        let want_continue = Confirm::new()
            .with_prompt(cyan.apply_to(prompt).to_string())
            .interact_on(&self.stdout.lock().unwrap())
            .unwrap_or_else(|e| panic!(format!("failed printing to stdout: {}", e)));

        if !want_continue {
            // anyhow::bail!("User cancelled operation");
            panic!("User cancelled operation");
        }

        *self.spacing_printed.lock().unwrap() = false;
    }

    pub fn input(&self, message: impl AsRef<str>) -> String {
        let cyan = Style::new().cyan();
        let level = Log::calculate_status_level(&self.statuses);

        let mut prompt = Log::create_line_prefix(level, PrefixPrefs::in_status().flag("?"));
        prompt += message.as_ref();

        *self.spacing_printed.lock().unwrap() = false;

        Input::new()
            .with_prompt(cyan.apply_to(prompt).to_string())
            .interact_on(&self.stdout.lock().unwrap())
            .unwrap_or_else(|e| panic!(format!("failed printing to stdout: {}", e)))
    }

    pub fn hidden_input(&self, message: impl AsRef<str>) -> String {
        let cyan = Style::new().cyan();
        let level = Log::calculate_status_level(&self.statuses);

        let mut prompt = Log::create_line_prefix(level, PrefixPrefs::in_status().flag("?"));
        prompt += message.as_ref();

        *self.spacing_printed.lock().unwrap() = false;

        Password::new()
            .with_prompt(cyan.apply_to(prompt).to_string())
            .interact_on(&self.stdout.lock().unwrap())
            .unwrap_or_else(|e| panic!(format!("failed printing to stdout: {}", e)))
    }

    pub fn choose(
        &self,
        message: impl AsRef<str>,
        items: impl IntoIterator<Item = impl ToString>,
        default_index: usize,
    ) -> usize {
        let cyan = Style::new().cyan();
        let items: Vec<_> = items.into_iter().collect();
        let level = Log::calculate_status_level(&self.statuses);

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
            .interact_on_opt(&self.stdout.lock().unwrap())
            .unwrap_or_else(|e| panic!(format!("failed printing to stdout: {}", e)));

        *self.spacing_printed.lock().unwrap() = false;

        if let Some(index) = index {
            index
        } else {
            // anyhow::bail!("User cancelled operation");
            panic!("User cancelled operation");
        }
    }

    fn print(out: &Mutex<Term>, msg: impl AsRef<str>) {
        let mut out = out.lock().unwrap();
        out.write(msg.as_ref().as_bytes()).unwrap_or_else(|e| {
            panic!(format!(
                "failed printing to {}: {}",
                get_term_label(out.target()),
                e
            ))
        });
        out.flush().unwrap_or_else(|e| {
            panic!(format!(
                "failed flushing to {}: {}",
                get_term_label(out.target()),
                e
            ))
        });
    }

    fn println(out: &Mutex<Term>, msg: impl AsRef<str>) {
        Log::print(out, msg);
        Log::print(out, "\n");
    }

    fn println_wrapped_text(
        stdout: &Arc<Mutex<Term>>,
        text: impl AsRef<str>,
        status_level: Option<usize>,
        first_line_prefix_prefs: PrefixPrefs,
        line_prefix_prefs: PrefixPrefs,
    ) {
        let prefix = Log::create_line_prefix(status_level, first_line_prefix_prefs);
        let prefix_len = prefix.char_count_without_styling();

        let text_len = text.as_ref().chars().count();
        let text_max_width = stdout
            .lock()
            .unwrap()
            .size_checked()
            .and_then(|s| (s.1 as usize).checked_sub(prefix_len))
            .filter(|l| *l > 0)
            .unwrap_or(text_len);
        let normalized_text = text.as_ref().normalize_styling();
        let mut text_chunks = normalized_text.wrapped_lines(text_max_width);

        let first_line = prefix + &text_chunks.next().unwrap_or_default();
        Log::println(&stdout, first_line);

        for chunk in text_chunks {
            let mut line = Log::create_line_prefix(status_level, line_prefix_prefs);
            line += &chunk;
            Log::println(&stdout, line);
        }
    }

    fn calculate_status_level(statuses: &Statuses) -> Option<usize> {
        Some(statuses.lock().unwrap().len())
            .filter(|i| *i > 0)
            .map(|i| i - 1)
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
    fn new(
        stdout: Arc<Mutex<Term>>,
        message: impl AsRef<str>,
        statuses: Statuses,
        failure: Arc<Mutex<bool>>,
        spacing_printed: Arc<Mutex<bool>>,
        tracking: bool,
    ) -> Self {
        let level = Log::calculate_status_level(&statuses)
            .map(|l| Some(l + 1))
            .unwrap_or(Some(0));

        Log::println_wrapped_text(
            &stdout,
            message,
            level,
            PrefixPrefs::with_connector("╓╴").flag("*"),
            PrefixPrefs::in_status_overflow(),
        );

        Log::println(
            &stdout,
            Log::create_line_prefix(level, PrefixPrefs::in_status_overflow()),
        );

        Status {
            stdout,
            statuses,
            failure,
            spacing_printed,
            tracking,
        }
    }
}

impl Drop for Status {
    fn drop(&mut self) {
        let level = Log::calculate_status_level(&self.statuses);

        let mut line = Log::create_line_prefix(level, PrefixPrefs::with_connector("╙─").flag("─"));
        if self.tracking {
            if !*self.failure.lock().unwrap() {
                line += &Style::new().green().apply_to("SUCCESS").to_string();
            } else {
                line += &Style::new().red().apply_to("FAILURE").to_string();
            }
        } else {
            line += "DONE";
        };

        if !*self.spacing_printed.lock().unwrap() {
            Log::println(
                &self.stdout,
                Log::create_line_prefix(level, PrefixPrefs::in_status_overflow()),
            );
        }

        Log::println(&self.stdout, line);

        Log::println(
            &self.stdout,
            Log::create_line_prefix(
                level.filter(|l| *l > 0).map(|l| l - 1),
                PrefixPrefs::in_status_overflow(),
            ),
        );
        *self.spacing_printed.lock().unwrap() = true;

        let mut statuses = self.statuses.lock().unwrap();
        statuses.pop();
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
