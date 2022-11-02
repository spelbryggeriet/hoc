use std::{
    fmt::Write as FmtWrite,
    io::{self, Stdout, Write as IoWrite},
    panic,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crossterm::{cursor, execute, queue, style, terminal, ExecutableCommand, QueueableCommand};
use log_facade::Level;
use once_cell::sync::OnceCell;
use spin_sleep::{SpinSleeper, SpinStrategy};

use super::{Log, ProgressLog, SimpleLog};
use crate::{log::Error, prelude::*};

mod animation;

pub fn init() {
    RenderThread::get_or_init();
}

#[throws(Error)]
pub fn cleanup() {
    RenderThread::get_or_init().terminate()?;
}

#[must_use]
pub struct RenderThread {
    handle: Mutex<Option<JoinHandle<Result<(), Error>>>>,
    wants_terminate: Arc<AtomicBool>,
    wants_pause: Arc<(Mutex<bool>, Condvar)>,
    is_paused: Arc<(Mutex<bool>, Condvar)>,
}

impl RenderThread {
    fn get_or_init() -> &'static RenderThread {
        static RENDER_THREAD: OnceCell<RenderThread> = OnceCell::new();

        RENDER_THREAD.get_or_init(RenderThread::new)
    }

    fn new() -> Self {
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

                            let mut logs = super::Progress::get_or_init().logs();
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
                            let (is_paused_mutex, is_paused_cvar) = &*is_paused;
                            let mut is_paused_lock =
                                is_paused_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);
                            *is_paused_lock = false;
                            is_paused_cvar.notify_one();
                        }
                    }

                    {
                        let mut stdout = io::stdout();
                        render_config.is_paused = false;

                        let mut logs = super::Progress::get_or_init().logs();
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

            let mut logs = super::Progress::get_or_init().logs();
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
            wants_terminate: wants_terminate_orig,
            wants_pause: wants_pause_orig,
            is_paused: is_paused_orig,
        }
    }

    #[throws(Error)]
    pub fn pause() -> PauseLock {
        let render_thread = Self::get_or_init();

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

        PauseLock
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

    #[throws(Error)]
    pub fn terminate(&self) {
        self.wants_terminate.store(true, Ordering::SeqCst);
        if let Some(thread_handle) = self.handle.lock().expect(EXPECT_THREAD_NOT_POSIONED).take() {
            thread_handle
                .join()
                .unwrap_or_else(|e| panic::resume_unwind(e))?;
        }
    }
}

#[must_use]
pub struct PauseLock;

impl Drop for PauseLock {
    fn drop(&mut self) {
        let render_thread = RenderThread::get_or_init();

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

impl SimpleLog {
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

impl ProgressLog {
    const RUNNING_COLOR: style::Color = style::Color::Yellow;
    const FINISHED_COLOR: style::Color = style::Color::DarkCyan;

    fn render_height(&self, is_paused: bool) -> usize {
        if self.logs.is_empty() && !is_paused {
            1
        } else {
            2 + self
                .logs
                .iter()
                .map(|log| match log {
                    Log::Simple(simple_log) => simple_log.render_height(),
                    Log::Progress(progress_log) => progress_log.render_height(is_paused),
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

        let run_time = *self.run_time.lock().expect(EXPECT_THREAD_NOT_POSIONED);
        let is_finished = run_time.is_some();

        let animation_state = if is_finished {
            animation::State::Finished
        } else if is_paused {
            animation::State::Paused
        } else {
            animation::State::Animating(animation_frame as isize)
        };
        let color = if is_finished {
            Self::FINISHED_COLOR
        } else {
            Self::RUNNING_COLOR
        };

        // Print indicator and progress message.
        queue!(
            stdout,
            cursor::MoveToColumn(2 * indentation as u16),
            style::SetForegroundColor(color),
            style::Print(animation::braille_spin(animation_state)),
            style::Print(" "),
            style::Print(&self.message),
        )?;

        let queue_elapsed = |s: &mut Stdout| -> Result<(), Error> {
            match run_time {
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
        let render_height = if render_single_line {
            if is_paused && !is_finished {
                queue!(
                    stdout,
                    style::Print("\n"),
                    cursor::MoveToColumn(2 * indentation as u16),
                    style::SetForegroundColor(color),
                    style::Print(animation::box_turn_swell(animation_state)),
                    style::Print("╶".repeat(width - 1)),
                )?;
                Some(2)
            } else {
                queue!(
                    stdout,
                    style::Print(" "),
                    style::Print(animation::separator_swell(animation_state)),
                    style::Print(" "),
                )?;
                queue_elapsed(stdout)?;
                Some(1)
            }
        } else {
            None
        };

        // Clear the rest of the line in case there is residues left from previous frame.
        queue!(
            stdout,
            style::SetForegroundColor(style::Color::Reset),
            terminal::Clear(terminal::ClearType::UntilNewLine),
        )?;

        if let Some(render_height) = render_height {
            return render_height;
        }

        // Reserve two rows for the header and the footer.
        let inner_max_height = max_height - 2;
        // Keep track of the number of render lines required for the submessages.
        let mut remaining_height = self.render_height(is_paused) - 2;

        // Keep track of an offset for animation frames.
        let mut rendered_logs_height = 0;

        let queue_prefix = |s: &mut Stdout, frame_offset: usize| -> Result<(), Error> {
            queue!(
                s,
                style::Print("\n"),
                cursor::MoveToColumn(2 * indentation as u16),
                style::SetForegroundColor(color),
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
                    let nested_height = progress_log.render_height(is_paused);

                    let rendered_height = if remaining_height - nested_height < inner_max_height {
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
            style::SetForegroundColor(color),
            style::Print(animation::box_turn_swell(
                animation_state.frame_offset(-2 * rendered_logs_height as isize)
            ))
        )?;

        if is_paused && !is_finished {
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

    #[throws(Error)]
    fn render_to_strings(&self, buffer: &mut StringBuffer, start_index: usize) -> usize {
        let elapsed = self
            .run_time
            .lock()
            .expect(EXPECT_THREAD_NOT_POSIONED)
            .expect("expected progress to be finished");

        let mut index = start_index;

        // Render indicator and progress message.
        let header_line = buffer.get_line_mut(index);

        write!(
            header_line,
            "{color}{icon} {message}",
            color = style::SetForegroundColor(Self::FINISHED_COLOR),
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
                    color = style::SetForegroundColor(ProgressLog::FINISHED_COLOR),
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
                        let height = progress.render_height(false);
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
                color = style::SetForegroundColor(Self::FINISHED_COLOR),
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
