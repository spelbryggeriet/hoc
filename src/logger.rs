use console::{Style, Term, TermTarget};
use dialoguer::{Confirm, Input, Password, Select};
use std::ops::Range;
use std::sync::{Arc, Mutex, Weak};

fn get_term_label(target: TermTarget) -> &'static str {
    match target {
        TermTarget::Stdout => "stdout",
        TermTarget::Stderr => "stderr",
    }
}

type Statuses = Arc<Mutex<Vec<Weak<Status>>>>;

pub struct Logger {
    stdout: Arc<Term>,
    statuses: Statuses,
}

pub struct Status {
    stdout: Arc<Term>,
    statuses: Statuses,
}

impl Logger {
    pub fn new() -> Self {
        Self {
            stdout: Arc::new(Term::buffered_stdout()),
            statuses: Arc::new(Mutex::new(vec![])),
        }
    }

    pub fn info(&self, message: impl AsRef<str>) {
        let white = Style::new().white();
        let level = Logger::calculate_status_level(&self.statuses);
        let mut line = Logger::create_line_prefix(level, "╟╴", white.apply_to("~").to_string()).0;
        line += message.as_ref();
        Logger::print(&self.stdout, line);
    }

    pub fn status(&mut self, message: impl AsRef<str>) -> Arc<Status> {
        let status = Arc::new(Status::new(
            Arc::clone(&self.stdout),
            message,
            Arc::clone(&self.statuses),
        ));
        self.statuses.lock().unwrap().push(Arc::downgrade(&status));
        status
    }

    pub fn _warning(&self, message: impl AsRef<str>) {
        let yellow = Style::new().yellow();
        Logger::print(
            &self.stdout,
            yellow
                .apply_to("⚠︎ ".to_string() + message.as_ref())
                .to_string(),
        );
    }

    pub fn error(&self, message: impl AsRef<str>) {
        let red = Style::new().red();
        Logger::print(
            &self.stdout,
            red.apply_to("⚠︎ ".to_string() + message.as_ref())
                .to_string(),
        );
    }

    pub fn prompt(&self, message: impl AsRef<str>) {
        let cyan = Style::new().cyan();

        let want_continue = Confirm::new()
            .with_prompt(
                cyan.apply_to("🤨 ".to_string() + message.as_ref())
                    .to_string(),
            )
            .interact_on(&self.stdout)
            .unwrap_or_else(|e| {
                panic!(format!(
                    "failed printing to {}: {}",
                    get_term_label(self.stdout.target()),
                    e
                ))
            });

        if !want_continue {
            // anyhow::bail!("User cancelled operation");
            panic!("User cancelled operation");
        }
    }

    pub fn input(&self, message: impl AsRef<str>) -> String {
        let yellow = Style::new().yellow();
        let level = Logger::calculate_status_level(&self.statuses);
        let mut prompt = Logger::create_line_prefix(level, "╟╴", yellow.apply_to("?").to_string()).0;
        prompt += &yellow.apply_to(message.as_ref()).to_string();

        Input::new()
            .with_prompt(prompt)
            .interact_on(&self.stdout)
            .unwrap_or_else(|e| {
                panic!(format!(
                    "failed printing to {}: {}",
                    get_term_label(self.stdout.target()),
                    e
                ))
            })
    }

    pub fn hidden_input(&self, message: impl AsRef<str>) -> String {
        let yellow = Style::new().yellow();
        let level = Logger::calculate_status_level(&self.statuses);
        let mut prompt = Logger::create_line_prefix(level, "╟╴", yellow.apply_to("?").to_string()).0;
        prompt += &yellow.apply_to(message.as_ref()).to_string();

        Password::new()
            .with_prompt(prompt)
            .interact_on(&self.stdout)
            .unwrap_or_else(|e| {
                panic!(format!(
                    "failed printing to {}: {}",
                    get_term_label(self.stdout.target()),
                    e
                ))
            })
    }

    pub fn choose(
        &self,
        message: impl AsRef<str>,
        items: impl IntoIterator<Item = impl ToString>,
        default_index: usize,
    ) -> usize {
        let cyan = Style::new().cyan();
        let items: Vec<_> = items.into_iter().collect();

        let index = Select::new()
            .with_prompt(
                cyan.apply_to("🤔 ".to_string() + message.as_ref())
                    .to_string(),
            )
            .items(&items)
            .default(default_index)
            .interact_on_opt(&self.stdout)
            .unwrap_or_else(|e| {
                panic!(format!(
                    "failed printing to {}: {}",
                    get_term_label(self.stdout.target()),
                    e
                ))
            });

        if let Some(index) = index {
            index
        } else {
            // anyhow::bail!("User cancelled operation");
            panic!("User cancelled operation");
        }
    }

    fn print(out: &Term, msg: impl AsRef<str>) {
        out.write_line(msg.as_ref()).unwrap_or_else(|e| {
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

    fn create_line_prefix(
        status_level: Option<usize>,
        connector: impl AsRef<str>,
        flag: impl AsRef<str>,
    ) -> (String, usize) {
        let mut line_prefix = String::new();
        let mut line_prefix_len = 0;

        if let Some(level) = status_level {
            for outer_level in 0..level {
                line_prefix += &Logger::get_status_level_color(outer_level)
                    .apply_to("│ ")
                    .to_string();
                line_prefix_len += 2;
            }

            line_prefix += &Logger::get_status_level_color(level)
                .apply_to(connector.as_ref())
                .to_string();
        } else {
            line_prefix += connector.as_ref();
        }

        line_prefix += flag.as_ref();
        line_prefix += " ";
        line_prefix_len += connector.as_ref().chars().count() + flag.as_ref().chars().count() + 1;

        (line_prefix, line_prefix_len)
    }
}

impl Status {
    fn new(stdout: Arc<Term>, message: impl AsRef<str>, statuses: Statuses) -> Self {
        let level = Logger::calculate_status_level(&statuses)
            .map(|l| Some(l + 1))
            .unwrap_or(Some(0));

        let (line_prefix, line_prefix_len) = Logger::create_line_prefix(level, "╔══", "");
        let message_len = message.as_ref().chars().count();
        let message_max_width = stdout
            .size_checked()
            .and_then(|s| (s.1 as usize).checked_sub(line_prefix_len))
            .filter(|l| *l > 0)
            .unwrap_or(message_len);

        let mut message_chunks = message.as_ref().words().word_lines(message_max_width);

        let line = line_prefix + message_chunks.next().unwrap_or_default();
        Logger::print(&stdout, line);

        for chunk in message_chunks {
            Logger::print(
                &stdout,
                Logger::create_line_prefix(level, "║  ", "").0 + chunk,
            );
        }

        Status { stdout, statuses }
    }
}

impl Drop for Status {
    fn drop(&mut self) {
        let green = Style::new().green();

        let level = Logger::calculate_status_level(&self.statuses);

        let mut line = String::new();
        line += &Logger::create_line_prefix(level, "╚══", "").0;
        line += &green.apply_to("SUCCESS").to_string();

        Logger::print(&self.stdout, line);

        let mut statuses = self.statuses.lock().unwrap();
        statuses.pop();
    }
}

trait StrExt {
    fn words(&self) -> Words;
}

impl StrExt for str {
    fn words(&self) -> Words {
        Words::new(self)
    }
}

#[derive(Default, Debug, Copy, Clone)]
struct Boundary {
    start: usize,
    end: usize,
}

impl Boundary {
    fn from_start(start: usize) -> Self {
        Self { start, end: start }
    }

    fn range(self) -> Range<usize> {
        self.start..self.end
    }

    fn distance(self) -> usize {
        assert!(
            self.start <= self.end,
            "start index {} is greater than end index {}",
            self.start,
            self.end
        );
        self.end - self.start
    }
}

#[derive(Default)]
struct Word<'a> {
    source: &'a str,
    bound: Boundary,
}

impl<'a> Word<'a> {
    fn get(&self) -> &'a str {
        &self.source[self.bound.range()]
    }
}

struct Words<'a> {
    source: &'a str,
    chars: std::str::Chars<'a>,
    word_bound: Boundary,
}

impl Default for Words<'_> {
    fn default() -> Self {
        Self {
            source: "",
            chars: "".chars(),
            word_bound: Boundary::default(),
        }
    }
}

impl<'a> Iterator for Words<'a> {
    type Item = Word<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let break_char = loop {
            match self.chars.next() {
                Some(c) if [' ', '-', ':', '/', ',', '.'].contains(&c) => break Some(c),
                Some(c) => self.word_bound.end += c.len_utf8(),
                None => {
                    if self.word_bound.distance() == 0 {
                        return None;
                    }
                    break None;
                }
            }
        };

        let start = self.word_bound.start;
        let end = self.word_bound.end + break_char.map(|c| c.len_utf8()).unwrap_or_default();
        self.word_bound = Boundary::from_start(end);

        Some(Word {
            source: self.source,
            bound: Boundary { start, end },
        })
    }
}

impl<'a> Words<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            chars: source.chars(),
            word_bound: Boundary::default(),
        }
    }

    fn word_lines(self, line_width: usize) -> WordLines<'a> {
        WordLines::new(self.source, self, line_width)
    }
}

struct WordLines<'a> {
    source: &'a str,
    words: Words<'a>,
    line_width: usize,
    line_bound: Boundary,
    overflow_word_bound: Option<Boundary>,
}

impl<'a> WordLines<'a> {
    fn new(source: &'a str, words: Words<'a>, line_width: usize) -> Self {
        assert!(line_width > 0, "line width must not be 0");
        WordLines {
            source,
            words,
            line_width,
            line_bound: Boundary::default(),
            overflow_word_bound: None,
        }
    }

    fn process_word(&mut self, maybe_bound: Option<Boundary>) -> Option<Option<&'a str>> {
        let maybe_line = match maybe_bound {
            // There is a gap between the words, so we will set an empty `Words` instance in
            // order to freeze any forthcoming words and stop processing any further lines.
            Some(bound) if bound.start != self.line_bound.end => {
                if self.line_bound.distance() == 0 {
                    self.words = Words::default();
                    None
                } else {
                    Some(&self.source[self.line_bound.range()])
                }
            }

            // The word is overflowing the current line, so finish the line and save the
            // overflowing word for future processing when the next line is requested.
            Some(bound) if self.line_bound.distance() + bound.distance() > self.line_width => {
                if self.line_bound.distance() == 0 {
                    // Line is empty, so the word needs to be broken up.
                    let line_end = self.line_bound.start + self.line_width;
                    let line = &self.source[self.line_bound.start..line_end];
                    self.overflow_word_bound = Some(Boundary {
                        start: line_end,
                        end: bound.end,
                    });
                    self.line_bound = Boundary::from_start(line_end);
                    Some(line)
                } else {
                    // We have processed words for this line previously, so we pass the whole word
                    // for overflow processing for future line requests.
                    let line = &self.source[self.line_bound.range()];
                    self.overflow_word_bound = Some(bound);
                    self.line_bound = Boundary::from_start(self.line_bound.end);
                    Some(line)
                }
            }

            // Word fits on the line, increase the current line boundary.
            Some(bound) => {
                self.line_bound.end = bound.end;
                return None;
            }

            // No words left, so finish the line if it was started.
            None => {
                if self.line_bound.distance() == 0 {
                    None
                } else {
                    let line = &self.source[self.line_bound.range()];
                    self.line_bound = Boundary::from_start(self.line_bound.end);
                    Some(line)
                }
            }
        };

        Some(maybe_line)
    }
}

impl<'a> Iterator for WordLines<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(bound) = self.overflow_word_bound.take() {
            // We have an overflowing word from the previous line to proceess.
            if let Some(maybe_line) = self.process_word(Some(bound)) {
                return maybe_line;
            }
        }

        loop {
            let maybe_bound = self.words.next().map(|w| w.bound);
            if let Some(maybe_line) = self.process_word(maybe_bound) {
                return maybe_line;
            }
        }
    }
}
