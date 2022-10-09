use std::{
    borrow::Cow,
    env,
    fmt::{self, Write as FmtWrite},
    fs::{self, File},
    io::{self, Stdout, Write as IoWrite},
    panic,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use crossterm::{cursor, execute, queue, style, terminal, QueueableCommand};
use lazy_static::lazy_static;
use log::{Level, LevelFilter, Log as LogTrait, Metadata, Record};
use spin_sleep::{SpinSleeper, SpinStrategy};
use thiserror::Error;

use crate::prelude::*;

const MAX_DEFAULT_LEVEL: Level = if cfg!(debug_assertions) {
    Level::Trace
} else {
    Level::Info
};

const EXPECT_THREAD_NOT_POSIONED: &'static str = "thread should not be poisoned";

lazy_static! {
    pub static ref PROGRESS_THREAD: ProgresHandler = ProgresHandler::init();
}

#[must_use]
pub struct ProgressHandle {
    start_time: Instant,
    is_finished: Arc<Mutex<Option<Duration>>>,
}

impl ProgressHandle {
    fn new(start_time: Instant, is_finished: Arc<Mutex<Option<Duration>>>) -> Self {
        Self {
            start_time,
            is_finished,
        }
    }

    pub fn finish(self) {}
}

impl Drop for ProgressHandle {
    fn drop(&mut self) {
        self.is_finished
            .lock()
            .expect(EXPECT_THREAD_NOT_POSIONED)
            .replace(self.start_time.elapsed());
        if let Err(err) = PROGRESS_THREAD.update_progresses() {
            panic!("{err}");
        };
    }
}

pub struct Logger {
    level: Level,
    start_time: DateTime<Utc>,
    buffer: Mutex<LoggerBuffer>,
    has_printed: Mutex<AtomicBool>,
}

impl Logger {
    #[throws(Error)]
    pub fn init() {
        lazy_static::initialize(&PROGRESS_THREAD);

        execute!(io::stdout(), cursor::Hide)?;

        let level_str = env::var("RUST_LOG")
            .map(|v| Cow::Owned(v.to_uppercase()))
            .unwrap_or(Cow::Borrowed(MAX_DEFAULT_LEVEL.as_str()));
        let level = match &*level_str {
            "E" | "ER" | "ERR" | "ERRO" | "ERROR" => Level::Error,
            "W" | "WA" | "WAR" | "WARN" | "WARNI" | "WARNIN" | "WARNING" => Level::Warn,
            "I" | "IN" | "INF" | "INFO" => Level::Info,
            "D" | "DE" | "DEB" | "DEBU" | "DEBUG" => Level::Debug,
            "T" | "TR" | "TRA" | "TRAC" | "TRACE" => Level::Trace,
            _ => throw!(Error::UnknownLevel(level_str.to_string())),
        };

        let logger = Self {
            level,
            start_time: Utc::now(),
            buffer: Mutex::new(LoggerBuffer::new()),
            has_printed: Mutex::new(AtomicBool::new(false)),
        };

        log::set_boxed_logger(Box::new(logger))?;
        log::set_max_level(LevelFilter::Trace);
    }

    #[throws(Error)]
    pub fn cleanup() {
        log::logger().flush();
        PROGRESS_THREAD.cleanup()?;

        execute!(io::stdout(), cursor::Show)?;
    }
}

impl LogTrait for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        let args_str = record.args().to_string();

        if self.enabled(record.metadata()) {
            let log = SimpleLog::new(record.level(), args_str.clone());

            let mut progress_lock = PROGRESS_THREAD
                .progress_log
                .lock()
                .expect(EXPECT_THREAD_NOT_POSIONED);
            if let Some((_, ref mut progress)) = *progress_lock {
                progress.push_simple_log(log);
            } else {
                let mut stdout = io::stdout();

                // Print new line from previous log.
                let has_printed_lock = self.has_printed.lock().expect(EXPECT_THREAD_NOT_POSIONED);
                if has_printed_lock.load(Ordering::SeqCst) {
                    stdout
                        .queue(style::Print("\n"))
                        .unwrap_or_else(|err| panic!("{err}"));
                } else {
                    has_printed_lock.store(true, Ordering::SeqCst)
                }

                // Render the simple log.
                log.render(&mut stdout)
                    .unwrap_or_else(|err| panic!("{err}"));

                // Flush to the screen.
                stdout.flush().unwrap_or_else(|err| panic!("{err}"));
            }
        }

        let mut buffer_lock = self.buffer.lock().expect(EXPECT_THREAD_NOT_POSIONED);
        buffer_lock.messages.push((
            LoggerMeta {
                timestamp: Utc::now(),
                level: record.level(),
                module: record.module_path().map(str::to_string),
            },
            args_str,
        ));

        if buffer_lock.messages.len() >= 100 {
            drop(buffer_lock);
            self.flush();
        }
    }

    fn flush(&self) {
        let home_dir = env::var("HOME").expect("HOME environment variable should exist");
        let log_dir = format!(
            "{home_dir}/.local/share/hoc/logs/{}",
            self.start_time.format("%Y/%m/%d"),
        );
        fs::create_dir_all(&log_dir).expect("directories should be able to be created");
        let mut file = File::options()
            .create(true)
            .append(true)
            .open(format!(
                "{log_dir}/{}.txt",
                self.start_time.format("%T.%6f")
            ))
            .expect("file should be unique");

        {
            let mut buffer_lock = self.buffer.lock().expect(EXPECT_THREAD_NOT_POSIONED);
            let mut longest_mod_name = buffer_lock.longest_mod_name.max(
                buffer_lock
                    .messages
                    .iter()
                    .filter_map(|(meta, _)| meta.module.as_ref().map(|module| module.len()))
                    .max()
                    .unwrap_or(0),
            );

            for (meta, message) in buffer_lock.messages.drain(..) {
                let res = if let Some(module) = &meta.module {
                    if module.len() > longest_mod_name {
                        longest_mod_name = module.len();
                    }

                    write!(
                        file,
                        "[{time:<27} {level:<7} {module:<longest_mod_name$}] {message}\n",
                        level = meta.level,
                        time = format!("{:?}", meta.timestamp),
                    )
                } else {
                    write!(
                        file,
                        "[{time:<27} {level:<7}{empty_mod:mod_len$}] {message}\n",
                        empty_mod = "",
                        level = meta.level,
                        mod_len = if longest_mod_name > 0 {
                            longest_mod_name + 1
                        } else {
                            0
                        },
                        time = format!("{:?}", meta.timestamp),
                    )
                };

                if let Err(err) = res {
                    panic!("{err}");
                }
            }

            buffer_lock.longest_mod_name = longest_mod_name;
        }
    }
}

struct LoggerBuffer {
    messages: Vec<(LoggerMeta, String)>,
    longest_mod_name: usize,
}

impl LoggerBuffer {
    const fn new() -> Self {
        Self {
            messages: Vec::new(),
            longest_mod_name: 0,
        }
    }
}

struct LoggerMeta {
    timestamp: DateTime<Utc>,
    level: Level,
    module: Option<String>,
}

#[must_use]
pub struct ProgresHandler {
    thread_handle: Mutex<Option<JoinHandle<Result<(), Error>>>>,
    progress_log: Arc<Mutex<Option<(usize, ProgressLog)>>>,
    wants_cancel: Arc<AtomicBool>,
}

impl ProgresHandler {
    pub fn init() -> Self {
        let progress_log_orig = Arc::new(Mutex::new(Option::<(usize, ProgressLog)>::None));
        let progress_log = Arc::clone(&progress_log_orig);

        let wants_cancel_orig = Arc::new(AtomicBool::new(false));
        let wants_cancel = Arc::clone(&wants_cancel_orig);

        let thread_handle = thread::spawn(move || {
            let spin_sleeper =
                SpinSleeper::new(100_000).with_spin_strategy(SpinStrategy::YieldThread);
            let mut frames = animation::frames();

            while !wants_cancel.load(Ordering::SeqCst) {
                let terminal_rows = terminal::size()?.1 as usize;

                {
                    let mut progress_lock = progress_log.lock().expect(EXPECT_THREAD_NOT_POSIONED);
                    if let Some((ref mut previous_height, ref mut progress)) = *progress_lock {
                        let mut stdout = io::stdout();

                        if *previous_height >= 2 {
                            stdout
                                .queue(cursor::MoveToPreviousLine(*previous_height as u16 - 1))?;
                        }
                        stdout.queue(cursor::MoveToColumn(0))?;

                        let frame = frames.next().expect("animation frames should be infinite");
                        *previous_height = progress.render(&mut stdout, terminal_rows, 0, frame)?;

                        stdout.flush()?;
                    }
                }

                spin_sleeper.sleep(Duration::new(0, 16_666_667));
            }

            Ok(())
        });

        Self {
            thread_handle: Mutex::new(Some(thread_handle)),
            progress_log: progress_log_orig,
            wants_cancel: wants_cancel_orig,
        }
    }

    pub fn push_progress(&self, message: String) -> ProgressHandle {
        let subprogress_log = ProgressLog::new(message);
        let mut progress_log_lock = self.progress_log.lock().expect(EXPECT_THREAD_NOT_POSIONED);
        if let Some((_, ref mut progress_log)) = *progress_log_lock {
            progress_log.push_progress_log(subprogress_log)
        } else {
            let handle = subprogress_log.get_handle();
            progress_log_lock.replace((0, subprogress_log));
            handle
        }
    }

    #[throws(Error)]
    pub fn update_progresses(&self) {
        let mut progress_log_lock = self.progress_log.lock().expect(EXPECT_THREAD_NOT_POSIONED);
        if let Some((previous_height, mut progress_log)) = progress_log_lock.take() {
            if progress_log.update() {
                let mut stdout = io::stdout();

                if previous_height >= 2 {
                    stdout.queue(cursor::MoveToPreviousLine(previous_height as u16 - 1))?;
                }
                stdout.queue(cursor::MoveToColumn(0))?;

                let mut buffer = StringBuffer::new();
                let rendered_height = progress_log.into_strings(&mut buffer, 0)?;
                for (i, line) in buffer.0.iter().enumerate().take(rendered_height) {
                    stdout.queue(style::Print(line))?;
                    stdout.queue(terminal::Clear(terminal::ClearType::UntilNewLine))?;

                    if i + 1 < rendered_height {
                        stdout.queue(style::Print("\n"))?;
                    }
                }

                stdout.flush()?;
            } else {
                progress_log_lock.replace((previous_height, progress_log));
            }
        }
    }

    #[throws(Error)]
    fn cleanup(&self) {
        self.wants_cancel.store(true, Ordering::SeqCst);
        if let Some(thread_handle) = self
            .thread_handle
            .lock()
            .expect(EXPECT_THREAD_NOT_POSIONED)
            .take()
        {
            thread_handle
                .join()
                .unwrap_or_else(|e| panic::resume_unwind(e))?;

            // Print a final new line.
            println!();
        }
    }
}

enum Log {
    Simple(SimpleLog),
    Progress(ProgressLog),
}

pub struct SimpleLog {
    level: Level,
    message: String,
}

impl SimpleLog {
    fn new(level: Level, message: String) -> Self {
        Self { level, message }
    }

    fn render_height(&self) -> usize {
        1
    }

    #[throws(Error)]
    fn render(&self, stdout: &mut Stdout) -> usize {
        let (level_icon, color) = self.get_icon_and_color();

        execute!(
            stdout,
            style::SetForegroundColor(color),
            style::Print(level_icon),
            style::Print(" "),
            style::Print(&self.message),
            style::SetForegroundColor(style::Color::Reset),
            terminal::Clear(terminal::ClearType::UntilNewLine),
        )?;

        1
    }

    #[throws(Error)]
    fn into_strings(self, buffer: &mut StringBuffer, index: usize) -> usize {
        let (level_icon, color) = self.get_icon_and_color();

        let line = buffer.get_line_mut(index);
        write!(
            line,
            "{color}{level_icon} {message}{reset}",
            color = style::SetForegroundColor(color),
            message = self.message,
            reset = style::SetForegroundColor(style::Color::Reset),
        )?;

        1
    }

    fn get_icon_and_color(&self) -> (char, style::Color) {
        match self.level {
            Level::Error => ('\u{f00d}', style::Color::Red),
            Level::Warn => ('\u{f12a}', style::Color::Yellow),
            Level::Info => ('\u{fcaf}', style::Color::White),
            Level::Debug => ('\u{fd2b}', style::Color::DarkMagenta),
            Level::Trace => ('\u{e241}', style::Color::DarkGrey),
        }
    }
}

pub struct ProgressLog {
    message: String,
    start_time: Instant,
    logs: Vec<Log>,
    is_finished: Arc<Mutex<Option<Duration>>>,
}

impl ProgressLog {
    const COLOR: style::Color = style::Color::DarkCyan;

    fn new(message: String) -> Self {
        Self {
            message,
            start_time: Instant::now(),
            logs: Vec::new(),
            is_finished: Arc::new(Mutex::new(None)),
        }
    }

    fn get_handle(&self) -> ProgressHandle {
        ProgressHandle::new(self.start_time, Arc::clone(&self.is_finished))
    }

    fn last_running_subprogress_mut(&mut self) -> Option<&mut Self> {
        self.logs
            .iter_mut()
            .filter_map(|log| {
                if let Log::Progress(progress_log) = log {
                    (!progress_log.is_finished()).then_some(progress_log)
                } else {
                    None
                }
            })
            .last()
    }

    fn push_simple_log(&mut self, simple_log: SimpleLog) {
        if let Some(last_running_subprogress) = self.last_running_subprogress_mut() {
            last_running_subprogress.push_simple_log(simple_log);
        } else {
            self.logs.push(Log::Simple(simple_log));
        }
    }

    fn push_progress_log(&mut self, progress_log: ProgressLog) -> ProgressHandle {
        if let Some(last_running_subprogress) = self.last_running_subprogress_mut() {
            last_running_subprogress.push_progress_log(progress_log)
        } else {
            let handle = progress_log.get_handle();
            self.logs.push(Log::Progress(progress_log));
            handle
        }
    }

    fn is_finished(&self) -> bool {
        self.is_finished
            .lock()
            .expect(EXPECT_THREAD_NOT_POSIONED)
            .is_some()
    }

    fn update(&mut self) -> bool {
        let mut subprogresses_finished = true;

        for log in self.logs.iter_mut() {
            if let Log::Progress(progress) = log {
                subprogresses_finished &= progress.update();
            }
        }

        let is_finished = self.is_finished();

        assert!(
            !(is_finished && !subprogresses_finished),
            r#"progress with message "{}" finished before all of its subprogresses finished"#,
            self.message,
        );

        is_finished
    }

    fn render_height(&self) -> usize {
        if self.logs.is_empty() {
            1
        } else {
            2 + self
                .logs
                .iter()
                .map(|log| match log {
                    Log::Simple(simple_log) => simple_log.render_height(),
                    Log::Progress(progress_log) => progress_log.render_height(),
                })
                .sum::<usize>()
        }
    }

    #[throws(Error)]
    fn render(
        &self,
        stdout: &mut Stdout,
        max_height: usize,
        indentation: usize,
        animation_frame: usize,
    ) -> usize {
        if max_height == 0 {
            return 0;
        }

        let is_finished = *self.is_finished.lock().expect(EXPECT_THREAD_NOT_POSIONED);

        // Print indicator and progress message.
        queue!(
            stdout,
            cursor::MoveToColumn(2 * indentation as u16),
            style::SetForegroundColor(Self::COLOR),
            style::Print(animation::braille_spin(
                is_finished.is_none().then_some(animation_frame as isize)
            )),
            style::Print(" "),
            style::Print(&self.message),
        )?;

        let queue_elapsed = |s: &mut Stdout| -> Result<(), Error> {
            match is_finished {
                None => {
                    let elapsed = self.start_time.elapsed();
                    queue!(
                        s,
                        style::Print(elapsed.as_secs()),
                        style::Print("."),
                        style::Print(elapsed.as_millis() % 1000 / 100),
                        style::Print("s"),
                    )?;
                }
                Some(elapsed) => queue!(
                    s,
                    style::Print(elapsed.as_secs()),
                    style::Print("."),
                    style::Print(format!("{:03}", elapsed.as_millis() % 1000)),
                    style::Print("s"),
                )?,
            }
            Ok(())
        };

        let render_single_line = self.logs.is_empty() || max_height == 1;

        // If there are no submessages, or if the max height is 1, print the elapsed time on the same
        // row as the progress message.
        if render_single_line {
            queue!(
                stdout,
                style::Print(" "),
                style::Print(animation::separator_swell(
                    is_finished.is_none().then_some(animation_frame as isize)
                )),
                style::Print(" "),
            )?;
            queue_elapsed(stdout)?;
        }

        // Clear the rest of the line in case there is residues left from previous frame.
        queue!(
            stdout,
            style::SetForegroundColor(style::Color::Reset),
            terminal::Clear(terminal::ClearType::UntilNewLine),
        )?;

        // Print submessages, if any.
        if render_single_line {
            1
        } else {
            // Reserve two rows for the header and the footer.
            let inner_max_height = max_height - 2;
            // Keep track of the number of render lines required for the submessages.
            let mut remaining_height = self.render_height() - 2;

            // Keep track of an offset for animation frames.
            let mut rendered_logs_height = 0;

            let queue_prefix = |s: &mut Stdout, frame_offset: usize| -> Result<(), Error> {
                queue!(
                    s,
                    style::Print("\n"),
                    cursor::MoveToColumn(2 * indentation as u16),
                    style::SetForegroundColor(Self::COLOR),
                    style::Print(animation::box_side_swell(
                        is_finished
                            .is_none()
                            .then_some(animation_frame as isize - 2 * frame_offset as isize)
                    )),
                    style::Print(" "),
                    style::SetForegroundColor(style::Color::Reset),
                )?;
                Ok(())
            };

            for log in self.logs.iter() {
                rendered_logs_height += match log {
                    Log::Simple(simple_log) => {
                        let rendered_lines = if remaining_height - 1 < inner_max_height {
                            queue_prefix(stdout, rendered_logs_height)?;

                            simple_log.render(stdout)?
                        } else {
                            0
                        };

                        remaining_height -= 1;

                        rendered_lines
                    }

                    Log::Progress(progress_log) => {
                        let nested_height = progress_log.render_height();

                        let rendered_height = if remaining_height - nested_height < inner_max_height
                        {
                            let truncated_nested_height = if remaining_height > inner_max_height {
                                nested_height - (remaining_height - inner_max_height)
                            } else {
                                nested_height
                            };

                            // Print prefix.
                            for i in 0..truncated_nested_height {
                                queue_prefix(stdout, rendered_logs_height + i)?;
                            }

                            // Reset cursor position.
                            if truncated_nested_height >= 2 {
                                stdout.queue(cursor::MoveToPreviousLine(
                                    truncated_nested_height as u16 - 1,
                                ))?;
                            }
                            stdout.queue(cursor::MoveToColumn(0))?;

                            progress_log.render(
                                stdout,
                                truncated_nested_height,
                                indentation + 1,
                                animation_frame,
                            )?
                        } else {
                            0
                        };

                        remaining_height -= nested_height;

                        rendered_height
                    }
                };
            }

            // Print elapsed time.
            queue!(
                stdout,
                style::Print("\n"),
                cursor::MoveToColumn(2 * indentation as u16),
                style::SetForegroundColor(Self::COLOR),
                style::Print(animation::box_turn_swell(is_finished.is_none().then_some(
                    animation_frame as isize - 2 * rendered_logs_height as isize
                ))),
                style::Print(animation::box_end_swell(is_finished.is_none().then_some(
                    animation_frame as isize - 2 * (rendered_logs_height as isize + 1)
                )))
            )?;
            queue_elapsed(stdout)?;
            queue!(
                stdout,
                terminal::Clear(terminal::ClearType::UntilNewLine),
                style::SetForegroundColor(style::Color::Reset),
            )?;

            2 + rendered_logs_height
        }
    }

    #[throws(Error)]
    fn into_strings(self, buffer: &mut StringBuffer, start_index: usize) -> usize {
        let elapsed = self
            .is_finished
            .lock()
            .expect(EXPECT_THREAD_NOT_POSIONED)
            .expect("expected progress to be finished");

        let mut index = start_index;

        // Render indicator and progress message.
        let header_line = buffer.get_line_mut(index);

        write!(
            header_line,
            "{color}{icon} {message}",
            color = style::SetForegroundColor(Self::COLOR),
            icon = animation::braille_spin(None),
            message = self.message,
        )?;

        let render_single_line = self.logs.is_empty();

        // If there are no submessages, render the elapsed time on the same row as the progress
        // message.
        if render_single_line {
            write!(
                header_line,
                " {separator} {secs}.{millis:03}s",
                separator = animation::separator_swell(None),
                secs = elapsed.as_secs(),
                millis = elapsed.as_millis() % 1000
            )?;
        }

        write!(
            header_line,
            "{reset}",
            reset = style::SetForegroundColor(style::Color::Reset),
        )?;
        index += 1;

        // Render submessages, if any.
        if !render_single_line {
            // Helper function to render prefixes for submessages.
            #[throws(Error)]
            fn render_prefix(buffer: &mut StringBuffer, index: usize) {
                let line = buffer.get_line_mut(index);

                write!(
                    line,
                    "{color}{side} {reset}",
                    color = style::SetForegroundColor(ProgressLog::COLOR),
                    reset = style::SetForegroundColor(style::Color::Reset),
                    side = animation::box_side_swell(None),
                )?;
            }

            for log in self.logs {
                index += match log {
                    Log::Simple(simple_log) => {
                        // Render prefix.
                        render_prefix(buffer, index)?;

                        simple_log.into_strings(buffer, index)?
                    }

                    Log::Progress(progress) => {
                        // Render prefix.
                        let height = progress.render_height();
                        for k in 0..height {
                            render_prefix(buffer, index + k)?;
                        }

                        progress.into_strings(buffer, index)?
                    }
                };
            }

            // Render elapsed time.
            let line = buffer.get_line_mut(index);
            write!(
                line,
                "{color}{turn}{end}{secs}.{millis:03}s{reset}",
                color = style::SetForegroundColor(Self::COLOR),
                end = animation::box_end_swell(None),
                millis = elapsed.as_millis() % 1000,
                reset = style::SetForegroundColor(style::Color::Reset),
                secs = elapsed.as_secs(),
                turn = animation::box_turn_swell(None),
            )?;
            index += 1;
        }

        index - start_index
    }
}

struct StringBuffer(Vec<String>);

impl StringBuffer {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn get_line_mut(&mut self, index: usize) -> &mut String {
        debug_assert!(
            self.0.len() >= index,
            "buffer length was `{}`, but it was expected to have a length of at least `{}`",
            self.0.len(),
            index + 1
        );

        if self.0.len() < index + 1 {
            self.0.push(String::new());
        }

        &mut self.0[index]
    }
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("Unknown log level '{0}'")]
    UnknownLevel(String),

    #[error("Failed to set logger: {0}")]
    SetLogger(#[from] log::SetLoggerError),

    #[error(transparent)]
    Crossterm(#[from] crossterm::ErrorKind),

    #[error(transparent)]
    Format(#[from] fmt::Error),
}

mod animation {
    const LENGTH: usize = 8;
    const SLOWDOWN: usize = 4;

    const BRAILLE_SPIN_ANIMATION: [char; LENGTH] = ['⢹', '⣸', '⣴', '⣦', '⣇', '⡏', '⠟', '⠻'];
    const BOX_SIDE_SWELL_ANIMATION: [char; LENGTH] = ['│', '╿', '┃', '┃', '┃', '┃', '╽', '│'];
    const BOX_TURN_SWELL_ANIMATION: [char; LENGTH] = ['└', '┖', '┗', '┗', '┗', '┗', '┕', '└'];
    const BOX_END_SWELL_ANIMATION: [char; LENGTH] = ['╴', '╸', '╸', '╸', '╸', '╸', '╴', '╴'];
    const SEPARATOR_SWELL_ANIMATION: [char; LENGTH] = ['─', '─', '─', '─', '─', '─', '─', '─'];

    const BRAILLE_SPIN_FINISHED: char = '';
    const BOX_SIDE_SWELL_FINISHED: char = '┃';
    const BOX_TURN_SWELL_FINISHED: char = '┗';
    const BOX_END_SWELL_FINISHED: char = '╸';
    const SEPARATOR_SWELL_FINISHED: char = '━';

    pub fn frames() -> impl Iterator<Item = usize> {
        (0..LENGTH).flat_map(|f| [f; SLOWDOWN]).cycle()
    }

    pub fn braille_spin(frame: Option<isize>) -> char {
        animate(frame, BRAILLE_SPIN_ANIMATION, BRAILLE_SPIN_FINISHED)
    }

    pub fn box_side_swell(frame: Option<isize>) -> char {
        animate(frame, BOX_SIDE_SWELL_ANIMATION, BOX_SIDE_SWELL_FINISHED)
    }

    pub fn box_turn_swell(frame: Option<isize>) -> char {
        animate(frame, BOX_TURN_SWELL_ANIMATION, BOX_TURN_SWELL_FINISHED)
    }

    pub fn box_end_swell(frame: Option<isize>) -> char {
        animate(frame, BOX_END_SWELL_ANIMATION, BOX_END_SWELL_FINISHED)
    }

    pub fn separator_swell(frame: Option<isize>) -> char {
        animate(frame, SEPARATOR_SWELL_ANIMATION, SEPARATOR_SWELL_FINISHED)
    }

    fn animate(frame: Option<isize>, animation_chars: [char; LENGTH], freeze_char: char) -> char {
        frame
            .map(|f| {
                let index = f.rem_euclid(LENGTH as isize) as usize;
                animation_chars[index]
            })
            .unwrap_or(freeze_char)
    }
}
