use std::{
    collections::VecDeque,
    fmt::{self, Write as FmtWrite},
    io::{self, Stdout, Write as IoWrite},
    panic,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossterm::{cursor, execute, queue, style, terminal, ExecutableCommand, QueueableCommand};
use log::Level;
use once_cell::sync::OnceCell;
use spin_sleep::{SpinSleeper, SpinStrategy};
use thiserror::Error;

use crate::prelude::*;

mod animation;

pub static RENDER_THREAD: OnceCell<RenderThread> = OnceCell::new();

pub fn progress(message: String) -> ProgressHandle {
    RENDER_THREAD
        .get()
        .expect(EXPECT_RENDER_THREAD_INITIALIZED)
        .push_progress(message)
}

#[throws(Error)]
pub fn pause() -> RenderThreadPauseLock {
    let render_thread = RENDER_THREAD.get().expect(EXPECT_RENDER_THREAD_INITIALIZED);

    {
        let (wants_pause_mutex, wants_pause_cvar) = &*render_thread.wants_pause;
        let mut wants_pause_lock = wants_pause_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);

        if *wants_pause_lock {
            throw!(Error::PauseLockAlreadyAcquired);
        }

        *wants_pause_lock = true;
        wants_pause_cvar.notify_one();
    }

    {
        let (is_paused_mutex, is_paused_cvar) = &*render_thread.is_paused;
        let is_paused_lock = is_paused_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);
        let _ = is_paused_cvar
            .wait_while(is_paused_lock, |is_paused| !*is_paused)
            .expect(EXPECT_THREAD_NOT_POSIONED);
    }

    println!();

    RenderThreadPauseLock
}

#[must_use]
pub struct RenderThread {
    handle: Mutex<Option<JoinHandle<Result<(), Error>>>>,
    logs: Arc<Mutex<VecDeque<Log>>>,
    wants_terminate: Arc<AtomicBool>,
    wants_pause: Arc<(Mutex<bool>, Condvar)>,
    is_paused: Arc<(Mutex<bool>, Condvar)>,
}

impl RenderThread {
    pub(super) fn init() -> Self {
        let logs_orig = Arc::new(Mutex::new(VecDeque::<Log>::new()));
        let logs = Arc::clone(&logs_orig);

        let wants_terminate_orig = Arc::new(AtomicBool::new(false));
        let wants_terminate = Arc::clone(&wants_terminate_orig);

        let wants_pause_orig = Arc::new((Mutex::new(false), Condvar::new()));
        let wants_pause = Arc::clone(&wants_pause_orig);

        let is_paused_orig = Arc::new((Mutex::new(false), Condvar::new()));
        let is_paused = Arc::clone(&is_paused_orig);

        let thread_handle = thread::spawn(move || {
            io::stdout().execute(cursor::Hide)?;

            let spin_sleeper =
                SpinSleeper::new(100_000).with_spin_strategy(SpinStrategy::YieldThread);
            let mut render_config = RenderConfig::new();

            while !wants_terminate.load(Ordering::SeqCst) {
                let (terminal_cols, terminal_rows) = terminal::size()?;
                render_config.width = terminal_cols as usize;
                render_config.max_running_progress_height = terminal_rows as usize;

                {
                    let (wants_pause_mutex, wants_pause_cvar) = &*wants_pause;
                    let wants_pause_lock =
                        wants_pause_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);

                    if *wants_pause_lock {
                        {
                            let mut stdout = io::stdout();
                            render_config.is_paused = true;

                            let mut logs_lock = logs.lock().expect(EXPECT_THREAD_NOT_POSIONED);
                            let logs = &mut *logs_lock;
                            while let Some(log) = logs.pop_front() {
                                match log {
                                    Log::Simple(ref simple_log) => {
                                        Self::print_simple_log(
                                            &mut stdout,
                                            &mut render_config,
                                            simple_log,
                                        )?;
                                    }

                                    Log::Progress(ref progress_log)
                                        if progress_log.is_finished() =>
                                    {
                                        Self::print_finished_progress_log(
                                            &mut stdout,
                                            &mut render_config,
                                            progress_log,
                                        )?;
                                    }

                                    Log::Progress(ref progress_log) => {
                                        Self::print_running_progress_log(
                                            &mut stdout,
                                            &mut render_config,
                                            progress_log,
                                        )?;
                                        logs.push_front(log);
                                        break;
                                    }
                                }

                                stdout.flush()?;
                            }
                        }

                        {
                            let (is_paused_mutex, is_paused_cvar) = &*is_paused;
                            let mut is_paused_lock =
                                is_paused_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);
                            *is_paused_lock = true;
                            is_paused_cvar.notify_one();
                        }

                        let _ = wants_pause_cvar
                            .wait_while(wants_pause_lock, |wants_pause| *wants_pause)
                            .expect(EXPECT_THREAD_NOT_POSIONED);

                        {
                            let (is_paused_mutex, is_paused_cvar) = &*RENDER_THREAD
                                .get()
                                .expect(EXPECT_RENDER_THREAD_INITIALIZED)
                                .is_paused;
                            let mut is_paused_lock =
                                is_paused_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);
                            *is_paused_lock = false;
                            is_paused_cvar.notify_one();
                        }
                    }

                    {
                        let mut stdout = io::stdout();
                        render_config.is_paused = false;

                        let mut logs_lock = logs.lock().expect(EXPECT_THREAD_NOT_POSIONED);
                        let logs = &mut *logs_lock;
                        while let Some(log) = logs.pop_front() {
                            match log {
                                Log::Simple(ref simple_log) => {
                                    Self::print_simple_log(
                                        &mut stdout,
                                        &mut render_config,
                                        simple_log,
                                    )?;
                                }
                                Log::Progress(ref progress_log) if !progress_log.is_finished() => {
                                    Self::print_running_progress_log(
                                        &mut stdout,
                                        &mut render_config,
                                        progress_log,
                                    )?;
                                    logs.push_front(log);
                                    break;
                                }
                                Log::Progress(ref progress_log) => {
                                    Self::print_finished_progress_log(
                                        &mut stdout,
                                        &mut render_config,
                                        progress_log,
                                    )?;
                                }
                            }

                            stdout.flush()?;
                        }
                    }
                }

                spin_sleeper.sleep(Duration::new(0, 16_666_667));
            }

            let mut stdout = io::stdout();

            let mut logs_lock = logs.lock().expect(EXPECT_THREAD_NOT_POSIONED);
            let logs = &mut *logs_lock;
            for log in logs.drain(..) {
                match log {
                    Log::Simple(ref simple_log) => {
                        Self::print_simple_log(&mut stdout, &mut render_config, simple_log)?;
                    }

                    Log::Progress(ref progress_log) if progress_log.is_finished() => {
                        Self::print_finished_progress_log(
                            &mut stdout,
                            &mut render_config,
                            progress_log,
                        )?;
                    }

                    _ => panic!("no progress should be running after cancellation"),
                }
                stdout.flush()?;
            }

            execute!(stdout, style::Print("\n"), cursor::Show)?;

            Ok(())
        });

        Self {
            handle: Mutex::new(Some(thread_handle)),
            logs: logs_orig,
            wants_terminate: wants_terminate_orig,
            wants_pause: wants_pause_orig,
            is_paused: is_paused_orig,
        }
    }

    #[throws(Error)]
    fn print_simple_log(
        stdout: &mut Stdout,
        render_config: &mut RenderConfig,
        simple_log: &SimpleLog,
    ) {
        // Print leading new line from previous progress log.
        if render_config.has_printed_finished_progress {
            println!()
        }

        // Print new line from previous log.
        if render_config.has_printed_non_running_progress_atleast_once {
            stdout.queue(style::Print("\n"))?;
        }

        // Render the simple log.
        simple_log.render(stdout)?;
        render_config.rendered_running_progress_height = 0;

        render_config.has_printed_non_running_progress_atleast_once = true;
        render_config.has_printed_finished_progress = false;
    }

    #[throws(Error)]
    fn print_running_progress_log(
        stdout: &mut Stdout,
        render_config: &mut RenderConfig,
        progress_log: &ProgressLog,
    ) {
        let starting_height = if render_config.has_printed_non_running_progress_atleast_once {
            println!();
            1
        } else {
            0
        };

        Self::clear_previous_running_progress_log(stdout, render_config)?;
        render_config.rendered_running_progress_height = starting_height;

        render_config.rendered_running_progress_height += progress_log.render(
            stdout,
            render_config.max_running_progress_height,
            render_config.width,
            0,
            render_config.next_frame(),
            render_config.is_paused,
        )?;

        if render_config.is_paused {
            println!();
            render_config.rendered_running_progress_height += 1;
        }

        render_config.has_printed_finished_progress = false;
    }

    #[throws(Error)]
    fn print_finished_progress_log(
        stdout: &mut Stdout,
        render_config: &mut RenderConfig,
        progress_log: &ProgressLog,
    ) {
        Self::clear_previous_running_progress_log(stdout, render_config)?;

        // Render finished progress to a vector of strings.
        let mut buffer = StringBuffer::new();
        let rendered_height = progress_log.render_to_strings(&mut buffer, 0)?;

        // Print rendered strings.
        if render_config.has_printed_non_running_progress_atleast_once {
            println!();
        }

        for (i, line) in buffer.0.iter().enumerate().take(rendered_height) {
            stdout.queue(style::Print(line))?;
            stdout.queue(terminal::Clear(terminal::ClearType::UntilNewLine))?;

            if i + 1 < rendered_height {
                stdout.queue(style::Print("\n"))?;
            }
        }

        render_config.has_printed_non_running_progress_atleast_once = true;
        render_config.has_printed_finished_progress = true;
    }

    #[throws(Error)]
    fn clear_previous_running_progress_log(stdout: &mut Stdout, render_config: &mut RenderConfig) {
        if render_config.rendered_running_progress_height >= 1 {
            if render_config.rendered_running_progress_height >= 2 {
                stdout.queue(cursor::MoveToPreviousLine(
                    render_config.rendered_running_progress_height as u16 - 1,
                ))?;
            }

            stdout.queue(cursor::MoveToColumn(0))?;
        } else if render_config.has_printed_non_running_progress_atleast_once {
            stdout.queue(style::Print("\n"))?;
        }

        render_config.rendered_running_progress_height = 0;
    }

    pub fn push_progress(&self, message: String) -> ProgressHandle {
        let subprogress_log = ProgressLog::new(message);

        // Find the current progress log.
        let mut logs_lock = self.logs.lock().expect(EXPECT_THREAD_NOT_POSIONED);
        let logs = &mut *logs_lock;
        let progress_log = logs.iter_mut().last().and_then(|log| {
            if let Log::Progress(progress_log) = log {
                (!progress_log.is_finished()).then_some(progress_log)
            } else {
                None
            }
        });

        if let Some(progress_log) = progress_log {
            progress_log.push_progress_log(subprogress_log)
        } else {
            let handle = subprogress_log.get_handle();
            logs.push_back(Log::Progress(subprogress_log));
            handle
        }
    }

    pub(super) fn push_simple_log(&self, level: Level, message: String) {
        // Find the current progress log.
        let mut logs_lock = RENDER_THREAD
            .get()
            .expect(EXPECT_RENDER_THREAD_INITIALIZED)
            .logs
            .lock()
            .expect(EXPECT_THREAD_NOT_POSIONED);
        let logs = &mut *logs_lock;
        let progress_log = logs.iter_mut().last().and_then(|log| {
            if let Log::Progress(progress_log) = log {
                (!progress_log.is_finished()).then_some(progress_log)
            } else {
                None
            }
        });

        if let Some(progress_log) = progress_log {
            for line in message.lines() {
                progress_log.push_simple_log(SimpleLog::new(level, line.to_string()));
            }
        } else {
            for line in message.lines() {
                logs.push_back(Log::Simple(SimpleLog::new(level, line.to_string())));
            }
        }
    }

    #[throws(Error)]
    pub(super) fn terminate(&self) {
        self.wants_terminate.store(true, Ordering::SeqCst);
        if let Some(thread_handle) = self.handle.lock().expect(EXPECT_THREAD_NOT_POSIONED).take() {
            thread_handle
                .join()
                .unwrap_or_else(|e| panic::resume_unwind(e))?;
        }
    }
}

#[must_use]
pub struct RenderThreadPauseLock;

impl Drop for RenderThreadPauseLock {
    fn drop(&mut self) {
        let render_thread = RENDER_THREAD.get().expect(EXPECT_RENDER_THREAD_INITIALIZED);

        {
            let (wants_pause_mutex, wants_pause_cvar) = &*render_thread.wants_pause;
            let mut wants_pause_lock = wants_pause_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);
            *wants_pause_lock = false;
            wants_pause_cvar.notify_one();
        }

        {
            let (is_paused_mutex, is_paused_cvar) = &*render_thread.is_paused;
            let is_paused_lock = is_paused_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);
            let _ = is_paused_cvar
                .wait_while(is_paused_lock, |is_paused| *is_paused)
                .expect(EXPECT_THREAD_NOT_POSIONED);
        }
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

struct RenderConfig {
    has_printed_non_running_progress_atleast_once: bool,
    has_printed_finished_progress: bool,
    is_paused: bool,
    rendered_running_progress_height: usize,
    max_running_progress_height: usize,
    width: usize,
    frames: animation::Frames,
}

impl RenderConfig {
    fn new() -> Self {
        Self {
            has_printed_non_running_progress_atleast_once: false,
            has_printed_finished_progress: false,
            is_paused: false,
            rendered_running_progress_height: 0,
            max_running_progress_height: 0,
            width: 0,
            frames: animation::Frames::new(),
        }
    }

    fn next_frame(&mut self) -> usize {
        self.frames
            .next()
            .expect("animation frames should be infinite")
    }
}

#[derive(Debug)]
enum Log {
    Simple(SimpleLog),
    Progress(ProgressLog),
}

#[derive(Debug)]
struct SimpleLog {
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
    fn render_to_strings(&self, buffer: &mut StringBuffer, index: usize) -> usize {
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

#[derive(Debug)]
struct ProgressLog {
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

    fn is_finished(&self) -> bool {
        self.is_finished
            .lock()
            .expect(EXPECT_THREAD_NOT_POSIONED)
            .is_some()
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
        width: usize,
        indentation: usize,
        animation_frame: usize,
        is_paused: bool,
    ) -> usize {
        if max_height == 0 {
            return 0;
        }

        let is_finished = *self.is_finished.lock().expect(EXPECT_THREAD_NOT_POSIONED);

        let animation_state = if is_finished.is_some() {
            animation::State::Finished
        } else if is_paused {
            animation::State::Paused
        } else {
            animation::State::Animating(animation_frame as isize)
        };

        // Print indicator and progress message.
        queue!(
            stdout,
            cursor::MoveToColumn(2 * indentation as u16),
            style::SetForegroundColor(Self::COLOR),
            style::Print(animation::braille_spin(animation_state)),
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
            if indentation == 0 && is_paused {
                queue!(
                    stdout,
                    style::Print("\n└"),
                    style::Print("╶".repeat(width - 1)),
                )?;
            } else {
                queue!(
                    stdout,
                    style::Print(" "),
                    style::Print(animation::separator_swell(animation_state)),
                    style::Print(" "),
                )?;
                queue_elapsed(stdout)?;
            }
        }

        // Clear the rest of the line in case there is residues left from previous frame.
        queue!(
            stdout,
            style::SetForegroundColor(style::Color::Reset),
            terminal::Clear(terminal::ClearType::UntilNewLine),
        )?;

        // Print submessages, if any.
        if render_single_line && is_paused {
            2
        } else if render_single_line {
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
                        animation_state.frame_offset(-2 * frame_offset as isize)
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
                                width.saturating_sub(2),
                                indentation + 1,
                                animation_frame,
                                is_paused,
                            )?
                        } else {
                            0
                        };

                        remaining_height -= nested_height;

                        rendered_height
                    }
                };
            }

            // Print prefix of elapsed line.
            queue!(
                stdout,
                style::Print("\n"),
                cursor::MoveToColumn(2 * indentation as u16),
                style::SetForegroundColor(Self::COLOR),
                style::Print(animation::box_turn_swell(
                    animation_state.frame_offset(-2 * rendered_logs_height as isize)
                ))
            )?;

            if is_paused {
                // Print dashed line to indicate paused, incomplete progress.
                stdout.queue(style::Print("╶".repeat(width - 1)))?;
            } else {
                // Print elapsed time.
                stdout.queue(style::Print(animation::box_end_swell(
                    animation_state.frame_offset(-2 * (rendered_logs_height as isize + 1)),
                )))?;
                queue_elapsed(stdout)?;
            }

            // Reset color and clear rest of line.
            queue!(
                stdout,
                style::SetForegroundColor(style::Color::Reset),
                terminal::Clear(terminal::ClearType::UntilNewLine),
            )?;

            2 + rendered_logs_height
        }
    }

    #[throws(Error)]
    fn render_to_strings(&self, buffer: &mut StringBuffer, start_index: usize) -> usize {
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
            icon = animation::braille_spin(animation::State::Finished),
            message = self.message,
        )?;

        let render_single_line = self.logs.is_empty();

        // If there are no submessages, render the elapsed time on the same row as the progress
        // message.
        if render_single_line {
            write!(
                header_line,
                " {separator} {secs}.{millis:03}s",
                separator = animation::separator_swell(animation::State::Finished),
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
                    side = animation::box_side_swell(animation::State::Finished),
                )?;
            }

            for log in self.logs.iter() {
                index += match log {
                    Log::Simple(simple_log) => {
                        // Render prefix.
                        render_prefix(buffer, index)?;

                        simple_log.render_to_strings(buffer, index)?
                    }

                    Log::Progress(progress) => {
                        // Render prefix.
                        let height = progress.render_height();
                        for k in 0..height {
                            render_prefix(buffer, index + k)?;
                        }

                        progress.render_to_strings(buffer, index)?
                    }
                };
            }

            // Render elapsed time.
            let line = buffer.get_line_mut(index);
            write!(
                line,
                "{color}{turn}{end}{secs}.{millis:03}s{reset}",
                color = style::SetForegroundColor(Self::COLOR),
                end = animation::box_end_swell(animation::State::Finished),
                millis = elapsed.as_millis() % 1000,
                reset = style::SetForegroundColor(style::Color::Reset),
                secs = elapsed.as_secs(),
                turn = animation::box_turn_swell(animation::State::Finished),
            )?;
            index += 1;
        }

        index - start_index
    }
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
    }
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("render thread pause lock already acquired")]
    PauseLockAlreadyAcquired,

    #[error(transparent)]
    Crossterm(#[from] crossterm::ErrorKind),

    #[error(transparent)]
    Format(#[from] fmt::Error),
}
