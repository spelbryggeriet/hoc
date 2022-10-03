use std::{
    borrow::Cow,
    env,
    fs::{self, File},
    io::{self, Stdout, Write},
    marker::PhantomData,
    mem, panic,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Barrier, Condvar, Mutex, RwLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use crossterm::{cursor, execute, queue, style, terminal, ExecutableCommand, QueueableCommand};
use lazy_static::lazy_static;
use log::{Level, LevelFilter, Log, Metadata, Record};
use spin_sleep::{SpinSleeper, SpinStrategy};
use thiserror::Error;

use crate::prelude::*;

const EXPECT_THREAD_NOT_POSIONED: &'static str = "thread should not be poisoned";

const MAX_DEFAULT_LEVEL: Level = if cfg!(debug_assertions) {
    Level::Trace
} else {
    Level::Info
};

lazy_static! {
    pub static ref PROGRESS: Arc<RwLock<Option<RunningProgress>>> = Arc::default();
    pub static ref PROGRESS_THREAD: RwLock<Option<ProgressThread>> =
        RwLock::new(Some(ProgressThread::init()));
}

pub fn __progress(message: String) -> ProgressHandle {
    let mut progress = PROGRESS.write().expect(EXPECT_THREAD_NOT_POSIONED);
    if let Some(ref mut progress) = *progress {
        progress.push_progress(message);
    } else {
        progress.replace(RunningProgress::new(message));
    }

    ProgressHandle(PhantomData::default())
}

#[must_use]
pub struct ProgressHandle(PhantomData<()>);

impl ProgressHandle {
    pub fn finish(self) {}
}

impl Drop for ProgressHandle {
    fn drop(&mut self) {
        let mut progress = PROGRESS.write().expect(EXPECT_THREAD_NOT_POSIONED);
        if !progress
            .as_mut()
            .expect("progress should be set if a handle exists")
            .pop_progress()
        {
            drop(progress);
            if let Some(progress_thread) =
                &*PROGRESS_THREAD.read().expect(EXPECT_THREAD_NOT_POSIONED)
            {
                let (pop_root_progress, barrier) = &*progress_thread.pop_root_progress;
                pop_root_progress.store(true, Ordering::SeqCst);
                barrier.wait();
            }
        }
    }
}

const ANIMATION_SLOWDOWN: usize = 4;
const ANIMATION_LENGTH: usize = 8;
const BRAILLE_SPIN_ANIMATION: ([char; ANIMATION_LENGTH], char) =
    (['⢹', '⣸', '⣴', '⣦', '⣇', '⡏', '⠟', '⠻'], '');
const BOX_SIDE_SWELL_ANIMATION: ([char; ANIMATION_LENGTH], char) =
    (['│', '╿', '┃', '┃', '┃', '┃', '╽', '│'], '┃');
const BOX_TURN_SWELL_ANIMATION: ([char; ANIMATION_LENGTH], char) =
    (['└', '┖', '┗', '┗', '┗', '┗', '┕', '└'], '┗');
const BOX_END_SWELL_ANIMATION: ([char; ANIMATION_LENGTH], char) =
    (['╴', '╸', '╸', '╸', '╸', '╸', '╴', '╴'], '╸');
const SEPARATOR_SWELL_ANIMATION: ([char; ANIMATION_LENGTH], char) = (['─'; ANIMATION_LENGTH], '━');

fn num_progress_render_lines(submessages: &[Submessage]) -> usize {
    if submessages.is_empty() {
        1
    } else {
        2 + submessages
            .iter()
            .map(|sm| match sm {
                Submessage::Raw(_) => 1,
                Submessage::Progress(
                    ProgressSubmessage::Running(RunningProgress { submessages, .. })
                    | ProgressSubmessage::Finished(FinishedProgress { submessages, .. }),
                ) => num_progress_render_lines(submessages),
            })
            .sum::<usize>()
    }
}

fn animate(
    (animation, default): ([char; ANIMATION_LENGTH], char),
    animation_frame: Option<isize>,
) -> char {
    animation_frame
        .map(|frame| {
            let wrapped_offset = frame.rem_euclid(ANIMATION_LENGTH as isize) as usize;
            animation[wrapped_offset]
        })
        .unwrap_or(default)
}

#[throws(Error)]
fn render_progress(
    stdout: &mut Stdout,
    mut max_rows: usize,
    animation_frame: Option<isize>,
    indentation: usize,
    message: &str,
    submessages: &[Submessage],
    queue_elapsed: impl Fn(&mut Stdout) -> Result<(), Error>,
) {
    // Print indicator and progress message.
    queue!(
        stdout,
        cursor::MoveToColumn(2 * indentation as u16),
        style::SetForegroundColor(style::Color::Yellow),
        style::Print(animate(BRAILLE_SPIN_ANIMATION, animation_frame)),
        style::Print(" "),
        style::Print(message),
    )?;

    // If there are no submessages, print the elapsed time on the same row as the progress message.
    if submessages.is_empty() {
        queue!(
            stdout,
            style::Print(" "),
            style::Print(animate(SEPARATOR_SWELL_ANIMATION, animation_frame)),
            style::Print(" "),
        )?;
        queue_elapsed(stdout)?;
        stdout.queue(style::Print("s"))?;
    }

    // Clear the rest of the line in case there is residues left from previous frame.
    queue!(
        stdout,
        style::SetForegroundColor(style::Color::Reset),
        terminal::Clear(terminal::ClearType::UntilNewLine),
    )?;

    // Print submessages, if any.
    if !submessages.is_empty() {
        // Reserve two rows for the header and the footer.
        max_rows -= 2;

        // Keep track of the number of render lines required for the submessages.
        let mut remaining_render_lines = num_progress_render_lines(submessages) - 2;

        // Keep track of an offset for animation frames.
        let mut frame_offset = 0;

        // Helper closure to queue prefixes for submessages.
        let queue_prefix = |s: &mut Stdout, frame_offset: isize| -> Result<(), Error> {
            Ok(queue!(
                s,
                style::Print("\n"),
                cursor::MoveToColumn(2 * indentation as u16),
                style::SetForegroundColor(style::Color::Yellow),
                style::Print(animate(
                    BOX_SIDE_SWELL_ANIMATION,
                    animation_frame.map(|f| f - 2 * frame_offset),
                )),
                style::Print(" "),
                style::SetForegroundColor(style::Color::Reset),
            )?)
        };

        for submessage in submessages.iter() {
            frame_offset += match submessage {
                Submessage::Raw(message) => {
                    if remaining_render_lines <= max_rows {
                        // Print prefix.
                        queue_prefix(stdout, frame_offset)?;

                        // Print progress message and clear previous frame residue.
                        queue!(
                            stdout,
                            style::Print(message),
                            terminal::Clear(terminal::ClearType::UntilNewLine),
                        )?;
                    }

                    remaining_render_lines -= 1;

                    1
                }
                Submessage::Progress(
                    nested_progress @ (ProgressSubmessage::Running(RunningProgress {
                        submessages,
                        ..
                    })
                    | ProgressSubmessage::Finished(FinishedProgress {
                        submessages,
                        ..
                    })),
                ) => {
                    let num_render_lines = num_progress_render_lines(submessages);

                    let nested_max_rows = if remaining_render_lines > max_rows {
                        num_render_lines.saturating_sub(remaining_render_lines - max_rows)
                    } else {
                        num_render_lines
                    };

                    // Print prefix.
                    for i in 0..nested_max_rows {
                        queue_prefix(stdout, frame_offset + i as isize)?;
                    }

                    // Reset cursor position.
                    if nested_max_rows > 1 {
                        stdout.queue(cursor::MoveToPreviousLine(nested_max_rows as u16 - 1))?;
                    }
                    stdout.queue(cursor::MoveToColumn(0))?;

                    match nested_progress {
                        ProgressSubmessage::Running(running_progress) => {
                            // Print nested running progress.
                            running_progress.render(
                                stdout,
                                nested_max_rows,
                                animation_frame,
                                indentation + 1,
                            )?;
                        }
                        ProgressSubmessage::Finished(finished_progress) => {
                            // Print nested finished progress.
                            finished_progress.render(stdout, nested_max_rows, indentation + 1)?;
                        }
                    }

                    remaining_render_lines -= num_render_lines;

                    num_render_lines as isize
                }
            };
        }

        if remaining_render_lines <= max_rows {
            // Print elapsed time.
            queue!(
                stdout,
                style::Print("\n"),
                cursor::MoveToColumn(2 * indentation as u16),
                style::SetForegroundColor(style::Color::Yellow),
                style::Print(animate(
                    BOX_TURN_SWELL_ANIMATION,
                    animation_frame.map(|f| f - 2 * frame_offset),
                )),
                style::Print(animate(
                    BOX_END_SWELL_ANIMATION,
                    animation_frame.map(|f| f - 2 * (frame_offset + 1)),
                )),
            )?;
            queue_elapsed(stdout)?;
            queue!(
                stdout,
                style::Print("s"),
                terminal::Clear(terminal::ClearType::UntilNewLine),
                style::SetForegroundColor(style::Color::Reset),
            )?;
        }
    }
}

pub struct Logger {
    level: Level,
    start_time: DateTime<Utc>,
    buffer: RwLock<LoggerBuffer>,
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
            buffer: RwLock::new(LoggerBuffer::new()),
        };

        log::set_boxed_logger(Box::new(logger))?;
        log::set_max_level(LevelFilter::Trace);
    }

    #[throws(Error)]
    pub fn cleanup() {
        log::logger().flush();

        if let Some(progress_thread) = PROGRESS_THREAD
            .write()
            .expect(EXPECT_THREAD_NOT_POSIONED)
            .take()
        {
            progress_thread.is_cancelled.store(true, Ordering::SeqCst);
            progress_thread
                .thread_handle
                .join()
                .unwrap_or_else(|e| panic::resume_unwind(e))?;
        }

        execute!(io::stdout(), cursor::Show)?;
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        let args_str = record.args().to_string();

        if self.enabled(record.metadata()) {
            let (level_icon, color) = match record.level() {
                Level::Error => ("\u{f00d}", style::Color::Red),
                Level::Warn => ("\u{f12a}", style::Color::Yellow),
                Level::Info => ("\u{fcaf}", style::Color::White),
                Level::Debug => ("\u{fd2b}", style::Color::DarkMagenta),
                Level::Trace => ("\u{e241}", style::Color::DarkGrey),
            };

            let mut log_line_bytes = Vec::new();
            execute!(
                log_line_bytes,
                style::SetForegroundColor(color),
                style::Print(format!("{level_icon} {args_str}")),
                style::SetForegroundColor(style::Color::Reset),
            )
            .expect("writing to `Vec` should always be successful");

            let log_line =
                String::from_utf8(log_line_bytes).expect("control sequence should be valid");

            let mut progress = PROGRESS.write().expect(EXPECT_THREAD_NOT_POSIONED);
            if let Some(progress) = &mut *progress {
                progress.push_log(log_line);
            } else {
                println!("{log_line}");
            }
        }

        let mut buffer = self.buffer.write().expect(EXPECT_THREAD_NOT_POSIONED);
        buffer.messages.push((
            LoggerMeta {
                timestamp: Utc::now(),
                level: record.level(),
                module: record.module_path().map(str::to_string),
            },
            args_str,
        ));

        if buffer.messages.len() > 100 {
            drop(buffer);
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
            let mut buffer = self.buffer.write().expect(EXPECT_THREAD_NOT_POSIONED);
            let mut longest_mod_name = buffer.longest_mod_name.max(
                buffer
                    .messages
                    .iter()
                    .filter_map(|(meta, _)| meta.module.as_ref().map(|module| module.len()))
                    .max()
                    .unwrap_or(0),
            );

            for (meta, message) in buffer.messages.drain(..) {
                let res = if let Some(module) = &meta.module {
                    if module.len() > longest_mod_name {
                        longest_mod_name = module.len();
                    }

                    file.write_fmt(format_args!(
                        "[{time:<27} {level:<7} {module:<longest_mod_name$}] {message}\n",
                        time = format!("{:?}", meta.timestamp),
                        level = meta.level,
                    ))
                } else {
                    file.write_fmt(format_args!(
                        "[{time:<27} {level:<7}{:mod_len$}] {message}\n",
                        "",
                        time = format!("{:?}", meta.timestamp),
                        level = meta.level,
                        mod_len = if longest_mod_name > 0 {
                            longest_mod_name + 1
                        } else {
                            0
                        },
                    ))
                };

                if let Err(err) = res {
                    panic!("{err}");
                }
            }

            buffer.longest_mod_name = longest_mod_name;
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
pub struct ProgressThread {
    thread_handle: JoinHandle<Result<(), Error>>,
    is_cancelled: Arc<AtomicBool>,
    pop_root_progress: Arc<(AtomicBool, Barrier)>,
}

impl ProgressThread {
    pub fn init() -> Self {
        let is_cancelled = Arc::new(AtomicBool::new(false));
        let pop_root_progress = Arc::new((AtomicBool::new(false), Barrier::new(2)));
        let should_cancel = Arc::clone(&is_cancelled);
        let should_pop_root_progress = Arc::clone(&pop_root_progress);
        let progress = Arc::clone(&PROGRESS);

        let thread_handle = thread::spawn(move || {
            let spin_sleeper =
                SpinSleeper::new(100_000).with_spin_strategy(SpinStrategy::YieldThread);
            let mut spin_symbol_iter = (0..ANIMATION_LENGTH)
                .flat_map(|f| [f; ANIMATION_SLOWDOWN])
                .cycle();

            let mut stdout = io::stdout();
            let mut previous_num_lines = 0;
            while !should_cancel.load(Ordering::SeqCst) {
                let terminal_rows = terminal::size()?.1 as usize;

                let (should_pop_root_progress, barrier) = &*should_pop_root_progress;
                if should_pop_root_progress.load(Ordering::SeqCst) {
                    let mut progress = progress.write().expect(EXPECT_THREAD_NOT_POSIONED);
                    if let Some(progress) = progress.take() {
                        let finished: FinishedProgress = progress.into();

                        if previous_num_lines > 1 {
                            stdout
                                .queue(cursor::MoveToPreviousLine(previous_num_lines as u16 - 1))?;
                        }
                        stdout.queue(cursor::MoveToColumn(0))?;

                        finished.render(&mut stdout, terminal_rows, 0)?;
                        stdout.queue(style::Print("\n"))?;
                    }
                    should_pop_root_progress.store(false, Ordering::SeqCst);
                    barrier.wait();
                }

                {
                    let progress = progress.read().expect(EXPECT_THREAD_NOT_POSIONED);
                    if let Some(ref progress) = *progress {
                        let (columns, _) = terminal::size()?;
                        stdout.queue(terminal::SetSize(columns, previous_num_lines as u16))?;
                        if previous_num_lines > 1 {
                            stdout
                                .queue(cursor::MoveToPreviousLine(previous_num_lines as u16 - 1))?;
                        }
                        stdout.queue(cursor::MoveToColumn(0))?;

                        let frame = spin_symbol_iter
                            .next()
                            .expect("spin symbol iterator should be infinite");
                        progress.render(&mut stdout, terminal_rows, Some(frame as isize), 0)?;
                        previous_num_lines =
                            num_progress_render_lines(&progress.submessages).min(terminal_rows);
                    }
                }

                stdout.flush()?;

                spin_sleeper.sleep(Duration::new(0, 16_666_667));
            }

            Ok(())
        });

        Self {
            thread_handle,
            is_cancelled,
            pop_root_progress,
        }
    }
}

enum Submessage {
    Raw(String),
    Progress(ProgressSubmessage),
}

enum ProgressSubmessage {
    Running(RunningProgress),
    Finished(FinishedProgress),
}

impl ProgressSubmessage {
    fn into_finished(&mut self) {
        match self {
            Self::Running(running) => {
                let dummy_running_progress = RunningProgress::new(String::default());
                let running = mem::replace(running, dummy_running_progress);
                mem::replace(self, ProgressSubmessage::Finished(running.into()));
            }
            _ => panic!("`ProgressLogType` should be the `Running` variant when converting into the `Finished`"),
        }
    }
}

pub struct RunningProgress {
    message: String,
    start_time: Instant,
    submessages: Vec<Submessage>,
}

impl RunningProgress {
    fn new(message: String) -> Self {
        Self {
            message,
            start_time: Instant::now(),
            submessages: Vec::new(),
        }
    }

    fn push_log(&mut self, message: String) {
        let last_running_progress = self
            .submessages
            .iter_mut()
            .filter(|sm| matches!(sm, Submessage::Progress(ProgressSubmessage::Running(_))))
            .last();
        if let Some(Submessage::Progress(ProgressSubmessage::Running(last_running_progress))) =
            last_running_progress
        {
            last_running_progress.push_log(message);
        } else {
            self.submessages.push(Submessage::Raw(message));
        }
    }

    fn push_progress(&mut self, message: String) {
        let last_nested_progress = self
            .submessages
            .iter_mut()
            .filter(|sm| matches!(sm, Submessage::Progress(_)))
            .last();
        if let Some(Submessage::Progress(ProgressSubmessage::Running(last_running_progress))) =
            last_nested_progress
        {
            last_running_progress.push_progress(message);
        } else {
            self.submessages
                .push(Submessage::Progress(ProgressSubmessage::Running(
                    RunningProgress::new(message),
                )));
        }
    }

    fn pop_progress(&mut self) -> bool {
        let last_running_progress = self
            .submessages
            .iter_mut()
            .filter(|sm| matches!(sm, Submessage::Progress(ProgressSubmessage::Running(_))))
            .last();
        if let Some(Submessage::Progress(progress)) = last_running_progress {
            if let ProgressSubmessage::Running(last_running_progress) = progress {
                if !last_running_progress.pop_progress() {
                    progress.into_finished();
                }
                true
            } else {
                unreachable!("matched progress should be running");
            }
        } else {
            false
        }
    }

    #[throws(Error)]
    fn render(
        &self,
        stdout: &mut Stdout,
        max_rows: usize,
        animation_frame: Option<isize>,
        indentation: usize,
    ) {
        let elapsed = self.start_time.elapsed();
        render_progress(
            stdout,
            max_rows,
            animation_frame,
            indentation,
            &self.message,
            &self.submessages,
            |s| {
                queue!(
                    s,
                    style::Print(elapsed.as_secs()),
                    style::Print("."),
                    style::Print(elapsed.as_millis() % 1000 / 100)
                )?;
                Ok(())
            },
        )?;
    }
}

pub struct FinishedProgress {
    message: String,
    elapsed: Duration,
    submessages: Vec<Submessage>,
}

impl FinishedProgress {
    #[throws(Error)]
    fn render(&self, stdout: &mut Stdout, max_rows: usize, indentation: usize) {
        render_progress(
            stdout,
            max_rows,
            None,
            indentation,
            &self.message,
            &self.submessages,
            |s| {
                queue!(
                    s,
                    style::Print(self.elapsed.as_secs()),
                    style::Print("."),
                    style::Print(self.elapsed.as_millis() % 1000)
                )?;
                Ok(())
            },
        )?
    }
}

impl From<RunningProgress> for FinishedProgress {
    fn from(running: RunningProgress) -> Self {
        Self {
            message: running.message,
            elapsed: running.start_time.elapsed(),
            submessages: running.submessages,
        }
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
}
