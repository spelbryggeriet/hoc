use std::{
    borrow::Cow,
    env,
    fmt::{self, Write as FmtWrite},
    fs::{self, File},
    io::{self, Stdout, Write as IoWrite},
    mem, panic,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use crossterm::{
    cursor, execute, queue,
    style::{self, Stylize},
    terminal, QueueableCommand,
};
use lazy_static::lazy_static;
use log::{Level, LevelFilter, Log, Metadata, Record};
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
    pub static ref PROGRESS_THREAD: ProgressThread = ProgressThread::init();
}

pub fn push_progress(message: String) -> ProgressHandle {
    PROGRESS_THREAD.sync();

    let mut progress = PROGRESS_THREAD.get_progress_mut();
    if let Some(ref mut progress) = *progress {
        progress.push_progress(message)
    } else {
        let running_progress = RunningProgress::new(message);
        let handle = running_progress.get_handle();
        progress.replace(running_progress);
        handle
    }
}

#[must_use]
pub struct ProgressHandle {
    should_finish: Arc<AtomicBool>,
}

impl ProgressHandle {
    fn new(should_finish: Arc<AtomicBool>) -> Self {
        Self { should_finish }
    }

    pub fn finish(self) {}
}

impl Drop for ProgressHandle {
    fn drop(&mut self) {
        self.should_finish.store(true, Ordering::SeqCst);
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

fn progress_render_height(submessages: &[Submessage]) -> usize {
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
                ) => progress_render_height(submessages),
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
    max_height: usize,
    indentation: usize,
    animation_frame: Option<isize>,
    message: &str,
    submessages: &[Submessage],
    queue_elapsed: impl Fn(&mut Stdout) -> Result<(), Error>,
) -> usize {
    if max_height == 0 {
        return 0;
    }

    // Print indicator and progress message.
    queue!(
        stdout,
        cursor::MoveToColumn(2 * indentation as u16),
        style::SetForegroundColor(style::Color::Yellow),
        style::Print(animate(BRAILLE_SPIN_ANIMATION, animation_frame)),
        style::Print(" "),
        style::Print(message),
    )?;

    let render_single_line = submessages.is_empty() || max_height == 1;

    // If there are no submessages, or if the max height is 1, print the elapsed time on the same
    // row as the progress message.
    if render_single_line {
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
    if !render_single_line {
        // Reserve two rows for the header and the footer.
        let inner_max_height = max_height - 2;

        // Keep track of the number of render lines required for the submessages.
        let mut remaining_height = progress_render_height(submessages) - 2;

        // Keep track of an offset for animation frames.
        let mut rendered_submessage_height = 0;

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
            rendered_submessage_height += match submessage {
                Submessage::Raw(message) => {
                    let rendered_lines = if remaining_height - 1 < inner_max_height {
                        // Print prefix.
                        queue_prefix(stdout, rendered_submessage_height as isize)?;

                        // Print progress message and clear previous frame residue.
                        queue!(
                            stdout,
                            style::Print(message),
                            terminal::Clear(terminal::ClearType::UntilNewLine),
                        )?;

                        1
                    } else {
                        0
                    };

                    remaining_height -= 1;

                    rendered_lines
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
                    let nested_height = progress_render_height(submessages);

                    let rendered_height = if remaining_height - nested_height < inner_max_height {
                        let truncated_nested_height = if remaining_height > inner_max_height {
                            nested_height - (remaining_height - inner_max_height)
                        } else {
                            nested_height
                        };

                        // Print prefix.
                        for i in 0..truncated_nested_height {
                            queue_prefix(stdout, (rendered_submessage_height + i) as isize)?;
                        }

                        // Reset cursor position.
                        if truncated_nested_height >= 2 {
                            stdout.queue(cursor::MoveToPreviousLine(
                                truncated_nested_height as u16 - 1,
                            ))?;
                        }
                        stdout.queue(cursor::MoveToColumn(0))?;

                        match nested_progress {
                            ProgressSubmessage::Running(running_progress) => {
                                // Print nested running progress.
                                running_progress.render(
                                    stdout,
                                    truncated_nested_height,
                                    animation_frame,
                                    indentation + 1,
                                )?
                            }
                            ProgressSubmessage::Finished(finished_progress) => {
                                // Print nested finished progress.
                                render_progress(
                                    stdout,
                                    truncated_nested_height,
                                    indentation + 1,
                                    None,
                                    &finished_progress.message,
                                    &finished_progress.submessages,
                                    finished_progress.get_queue_elapsed(),
                                )?
                            }
                        }
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
            style::SetForegroundColor(style::Color::Yellow),
            style::Print(animate(
                BOX_TURN_SWELL_ANIMATION,
                animation_frame.map(|f| f - 2 * rendered_submessage_height as isize),
            )),
            style::Print(animate(
                BOX_END_SWELL_ANIMATION,
                animation_frame.map(|f| f - 2 * (rendered_submessage_height as isize + 1)),
            )),
        )?;
        queue_elapsed(stdout)?;
        queue!(
            stdout,
            style::Print("s"),
            terminal::Clear(terminal::ClearType::UntilNewLine),
            style::SetForegroundColor(style::Color::Reset),
        )?;

        2 + rendered_submessage_height
    } else {
        1
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
        PROGRESS_THREAD.cleanup()?;

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

            PROGRESS_THREAD.sync();

            let mut progress = PROGRESS_THREAD.get_progress_mut();
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
    progress: Arc<RwLock<Option<RunningProgress>>>,
    should_sync: Arc<(Mutex<bool>, Condvar)>,
    should_cancel: Arc<AtomicBool>,
    thread_handle: RwLock<Option<JoinHandle<Result<(), Error>>>>,
}

impl ProgressThread {
    pub fn init() -> Self {
        let progress_orig = Arc::new(RwLock::new(Option::<RunningProgress>::None));
        let should_resync_orig = Arc::new((Mutex::new(false), Condvar::new()));
        let should_cancel_orig = Arc::new(AtomicBool::new(false));

        let progress = Arc::clone(&progress_orig);
        let should_resync = Arc::clone(&should_resync_orig);
        let should_cancel = Arc::clone(&should_cancel_orig);

        let thread_handle = thread::spawn(move || {
            let spin_sleeper =
                SpinSleeper::new(100_000).with_spin_strategy(SpinStrategy::YieldThread);
            let mut spin_symbol_iter = (0..ANIMATION_LENGTH)
                .flat_map(|f| [f; ANIMATION_SLOWDOWN])
                .cycle();

            let mut stdout = io::stdout();
            let mut previous_height: usize = 0;
            while !should_cancel.load(Ordering::SeqCst) {
                let terminal_rows = terminal::size()?.1 as usize;

                {
                    let mut progress_opt = progress.write().expect(EXPECT_THREAD_NOT_POSIONED);
                    if let Some(mut progress) = progress_opt.take() {
                        progress.finish_progresses();
                        if progress.should_finish() {
                            if previous_height >= 2 {
                                stdout.queue(cursor::MoveToPreviousLine(
                                    previous_height as u16 - 1,
                                ))?;
                            }
                            stdout.queue(cursor::MoveToColumn(0))?;
                            previous_height = 0;

                            let finished: FinishedProgress = progress.into();
                            let mut buffer = Vec::new();
                            finished.render_to_strings(&mut buffer, 0)?;
                            for line in buffer {
                                stdout.queue(style::Print(line))?;
                                stdout.queue(terminal::Clear(terminal::ClearType::UntilNewLine))?;
                                stdout.queue(style::Print("\n"))?;
                            }

                            *should_resync.0.lock().expect(EXPECT_THREAD_NOT_POSIONED) = false;
                            should_resync.1.notify_one();
                        } else {
                            progress_opt.replace(progress);
                        }
                    }
                }

                {
                    let progress = progress.read().expect(EXPECT_THREAD_NOT_POSIONED);
                    if let Some(ref progress) = *progress {
                        let (columns, _) = terminal::size()?;
                        stdout.queue(terminal::SetSize(columns, previous_height as u16))?;
                        if previous_height >= 2 {
                            stdout.queue(cursor::MoveToPreviousLine(previous_height as u16 - 1))?;
                        }
                        stdout.queue(cursor::MoveToColumn(0))?;

                        let frame = spin_symbol_iter
                            .next()
                            .expect("spin symbol iterator should be infinite");
                        previous_height =
                            progress.render(&mut stdout, terminal_rows, Some(frame as isize), 0)?;
                    }
                }

                stdout.flush()?;

                spin_sleeper.sleep(Duration::new(0, 16_666_667));
            }

            Ok(())
        });

        Self {
            progress: progress_orig,
            should_sync: should_resync_orig,
            should_cancel: should_cancel_orig,
            thread_handle: RwLock::new(Some(thread_handle)),
        }
    }

    fn get_progress_mut(&self) -> RwLockWriteGuard<Option<RunningProgress>> {
        self.progress.write().expect(EXPECT_THREAD_NOT_POSIONED)
    }

    fn get_progress(&self) -> RwLockReadGuard<Option<RunningProgress>> {
        self.progress.read().expect(EXPECT_THREAD_NOT_POSIONED)
    }

    fn sync(&self) {
        let mut lock = self.should_sync.0.lock().expect(EXPECT_THREAD_NOT_POSIONED);
        *lock = self
            .get_progress()
            .as_ref()
            .map(RunningProgress::should_finish)
            .unwrap_or(false);
        let _ = self
            .should_sync
            .1
            .wait_while(lock, |should_sync| *should_sync)
            .expect(EXPECT_THREAD_NOT_POSIONED);
    }

    #[throws(Error)]
    fn cleanup(&self) {
        PROGRESS_THREAD.sync();

        self.should_cancel.store(true, Ordering::SeqCst);
        if let Some(thread_handle) = self
            .thread_handle
            .write()
            .expect(EXPECT_THREAD_NOT_POSIONED)
            .take()
        {
            thread_handle
                .join()
                .unwrap_or_else(|e| panic::resume_unwind(e))?;
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
                *self = ProgressSubmessage::Finished(running.into());
            }
            _ => panic!("`ProgressLogType` should be the `Running` variant when converting into the `Finished`"),
        }
    }
}

pub struct RunningProgress {
    message: String,
    start_time: Instant,
    submessages: Vec<Submessage>,
    should_finish: Arc<AtomicBool>,
}

impl RunningProgress {
    fn new(message: String) -> Self {
        Self {
            message,
            start_time: Instant::now(),
            submessages: Vec::new(),
            should_finish: Arc::new(AtomicBool::new(false)),
        }
    }

    fn get_handle(&self) -> ProgressHandle {
        ProgressHandle::new(Arc::clone(&self.should_finish))
    }

    fn push_log(&mut self, message: String) {
        let last_running_progress = self
            .submessages
            .iter_mut()
            .filter(|sm| matches!(sm, Submessage::Progress(ProgressSubmessage::Running(rp)) if !rp.should_finish()))
            .last();
        if let Some(Submessage::Progress(ProgressSubmessage::Running(last_running_progress))) =
            last_running_progress
        {
            last_running_progress.push_log(message);
        } else {
            self.submessages.push(Submessage::Raw(message));
        }
    }

    fn push_progress(&mut self, message: String) -> ProgressHandle {
        let last_nested_progress = self
            .submessages
            .iter_mut()
            .filter(|sm| {
                matches!(
                    sm,
                    Submessage::Progress(ProgressSubmessage::Running(rp))
                    if !rp.should_finish()
                )
            })
            .last();
        if let Some(Submessage::Progress(ProgressSubmessage::Running(last_running_progress))) =
            last_nested_progress
        {
            last_running_progress.push_progress(message)
        } else {
            let running_progress = RunningProgress::new(message);
            let handle = running_progress.get_handle();
            self.submessages
                .push(Submessage::Progress(ProgressSubmessage::Running(
                    running_progress,
                )));
            handle
        }
    }

    fn should_finish(&self) -> bool {
        self.should_finish.load(Ordering::SeqCst)
    }

    fn finish_progresses(&mut self) {
        for sm in self.submessages.iter_mut() {
            if let Submessage::Progress(progress) = sm {
                if let ProgressSubmessage::Running(running_progress) = progress {
                    running_progress.finish_progresses();
                    if running_progress.should_finish() {
                        progress.into_finished();
                    }
                }
            }
        }
    }

    #[throws(Error)]
    fn render(
        &self,
        stdout: &mut Stdout,
        max_height: usize,
        animation_frame: Option<isize>,
        indentation: usize,
    ) -> usize {
        let elapsed = self.start_time.elapsed();
        render_progress(
            stdout,
            max_height,
            indentation,
            animation_frame,
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
        )?
    }
}

pub struct FinishedProgress {
    message: String,
    elapsed: Duration,
    submessages: Vec<Submessage>,
}

impl FinishedProgress {
    #[throws(Error)]
    fn render_to_strings(&self, buffer: &mut Vec<String>, start_index: usize) -> usize {
        let mut index = start_index;

        // Helper function to get a line from the buffer, or create it if it does not exist.
        fn get_line_mut(buffer: &mut Vec<String>, index: usize) -> &mut String {
            debug_assert!(
                buffer.len() >= index,
                "buffer length was `{}`, but it was expected to have a length of at least `{}`",
                buffer.len(),
                index + 1
            );

            if buffer.len() < index + 1 {
                buffer.push(String::new());
            }
            &mut buffer[index]
        }

        // Render indicator and progress message.
        let header_line = get_line_mut(buffer, index);

        write!(
            header_line,
            "{color}{icon} {message}",
            color = style::SetForegroundColor(style::Color::Yellow),
            icon = BRAILLE_SPIN_ANIMATION.1,
            message = self.message,
        )?;

        let render_single_line = self.submessages.is_empty();

        // If there are no submessages, render the elapsed time on the same row as the progress
        // message.
        if render_single_line {
            write!(
                header_line,
                " {separator} {secs}.{millis:03}s",
                separator = SEPARATOR_SWELL_ANIMATION.1,
                secs = self.elapsed.as_secs(),
                millis = self.elapsed.as_millis() % 1000
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
            fn render_prefix(buffer: &mut Vec<String>, index: usize) -> &mut String {
                let line = get_line_mut(buffer, index);

                write!(
                    line,
                    "{color}{side} {reset}",
                    color = style::SetForegroundColor(style::Color::Yellow),
                    reset = style::SetForegroundColor(style::Color::Reset),
                    side = BOX_SIDE_SWELL_ANIMATION.1,
                )?;

                line
            }

            for submessage in self.submessages.iter() {
                index += match submessage {
                    Submessage::Raw(message) => {
                        // Render prefix.
                        let line = render_prefix(buffer, index)?;

                        // Render progress message.
                        write!(line, "{message}")?;

                        1
                    }
                    Submessage::Progress(ProgressSubmessage::Finished(finished_progress)) => {
                        // Render prefix.
                        let height = progress_render_height(&finished_progress.submessages);
                        for k in 0..height {
                            render_prefix(buffer, index + k)?;
                        }

                        // Render nested finished progress.
                        let real_height = finished_progress.render_to_strings(buffer, index)?;

                        debug_assert!(
                            height == real_height,
                            "expected rendered height to be `{height}`, got `{real_height}`"
                        );

                        real_height
                    }
                    Submessage::Progress(ProgressSubmessage::Running(_)) => {
                        panic!("a finished progress cannot contain a running progress")
                    }
                };
            }

            // Render elapsed time.
            let line = get_line_mut(buffer, index);
            write!(
                line,
                "{color}{turn}{end}{secs}.{millis:03}s{reset}",
                color = style::SetForegroundColor(style::Color::Yellow),
                end = BOX_END_SWELL_ANIMATION.1,
                millis = self.elapsed.as_millis() % 1000,
                reset = style::SetForegroundColor(style::Color::Reset),
                secs = self.elapsed.as_secs(),
                turn = BOX_TURN_SWELL_ANIMATION.1,
            )?;
            index += 1;
        }

        index - start_index
    }

    fn get_queue_elapsed(&self) -> impl Fn(&mut Stdout) -> Result<(), Error> + '_ {
        |stdout| {
            queue!(
                stdout,
                style::Print(self.elapsed.as_secs()),
                style::Print("."),
                style::Print(format!("{:03}", self.elapsed.as_millis() % 1000)),
            )?;
            Ok(())
        }
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

    #[error(transparent)]
    Format(#[from] fmt::Error),
}
