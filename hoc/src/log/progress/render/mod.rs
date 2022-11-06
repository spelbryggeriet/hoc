use std::{
    io, panic,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crossterm::{cursor, execute, style, terminal, ExecutableCommand};
use log_facade::Level;
use once_cell::sync::OnceCell;
use spin_sleep::{SpinSleeper, SpinStrategy};

use self::view::{Position, RootView, View};
use super::{Log, ProgressLog, SimpleLog};
use crate::{log::Error, prelude::*};

mod animation;
mod view;

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
            let mut render_info = RenderInfo::new();
            let mut previous_height = None;

            let (terminal_cols, _) = terminal::size()?;
            let mut view = RootView::new(terminal_cols as usize);

            while !wants_terminate.load(Ordering::SeqCst) {
                let (terminal_cols, terminal_rows) = terminal::size()?;

                view.set_max_width(terminal_cols as usize);
                view.set_infinite_height();

                {
                    let (wants_pause_mutex, wants_pause_cvar) = &*wants_pause;
                    let wants_pause_lock =
                        wants_pause_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);

                    if *wants_pause_lock {
                        {
                            render_info.is_paused = true;

                            let mut logs = super::Progress::get_or_init().logs();
                            while let Some(log) = logs.pop_front() {
                                match log {
                                    Log::Simple(ref simple_log) => {
                                        Self::print_simple_log(
                                            &mut view,
                                            &mut render_info,
                                            simple_log,
                                            &mut previous_height,
                                        )?;
                                    }

                                    Log::Progress(ref progress_log) => {
                                        let is_running = !progress_log.is_finished();
                                        if is_running {
                                            view.set_max_height(terminal_rows as usize);
                                        }

                                        Self::print_progress_log(
                                            &mut view,
                                            &mut render_info,
                                            progress_log,
                                            &mut previous_height,
                                        )?;

                                        if is_running {
                                            logs.push_front(log);
                                            break;
                                        }
                                    }
                                }

                                view.set_infinite_height();
                            }
                        }

                        {
                            let (is_paused_mutex, is_paused_cvar) = &*is_paused;
                            let mut is_paused_lock =
                                is_paused_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);
                            *is_paused_lock = true;
                            is_paused_cvar.notify_one();
                        }

                        {
                            let _lock = wants_pause_cvar
                                .wait_while(wants_pause_lock, |wants_pause| *wants_pause)
                                .expect(EXPECT_THREAD_NOT_POSIONED);
                        }

                        {
                            let (is_paused_mutex, is_paused_cvar) = &*is_paused;
                            let mut is_paused_lock =
                                is_paused_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);
                            *is_paused_lock = false;
                            is_paused_cvar.notify_one();
                        }
                    }

                    {
                        view.set_max_width(terminal_cols as usize);
                        view.set_infinite_height();

                        render_info.is_paused = false;

                        let mut logs = super::Progress::get_or_init().logs();
                        while let Some(log) = logs.pop_front() {
                            match log {
                                Log::Simple(ref simple_log) => {
                                    Self::print_simple_log(
                                        &mut view,
                                        &mut render_info,
                                        simple_log,
                                        &mut previous_height,
                                    )?;
                                }

                                Log::Progress(ref progress_log) => {
                                    let is_running = !progress_log.is_finished();
                                    if is_running {
                                        view.set_max_height(terminal_rows as usize);
                                    }

                                    Self::print_progress_log(
                                        &mut view,
                                        &mut render_info,
                                        progress_log,
                                        &mut previous_height,
                                    )?;

                                    if is_running {
                                        logs.push_front(log);
                                        break;
                                    }
                                }
                            }

                            view.set_infinite_height();
                        }
                    }
                }

                spin_sleeper.sleep(Duration::new(0, 16_666_667));
            }

            let (terminal_cols, _) = terminal::size()?;

            view.set_max_width(terminal_cols as usize);
            view.set_infinite_height();

            let mut logs = super::Progress::get_or_init().logs();
            for log in logs.drain(..) {
                match log {
                    Log::Simple(ref simple_log) => {
                        Self::print_simple_log(
                            &mut view,
                            &mut render_info,
                            simple_log,
                            &mut previous_height,
                        )?;
                    }

                    Log::Progress(ref progress_log) if progress_log.is_finished() => {
                        Self::print_progress_log(
                            &mut view,
                            &mut render_info,
                            progress_log,
                            &mut previous_height,
                        )?;
                    }

                    _ => panic!("no progress should be running after cancellation"),
                }
            }

            execute!(io::stdout(), style::Print("\n"), cursor::Show)?;

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
            let _lock = is_paused_cvar
                .wait_while(is_paused_lock, |is_paused| !*is_paused)
                .expect(EXPECT_THREAD_NOT_POSIONED);
        }

        println!();

        PauseLock
    }

    #[throws(Error)]
    fn print_simple_log(
        view: &mut RootView,
        render_info: &mut RenderInfo,
        simple_log: &SimpleLog,
        previous_height: &mut Option<usize>,
    ) {
        let prepadding = render_info
            .previous_log_type
            .map(|t| if t != LogType::Simple { 2 } else { 1 })
            .unwrap_or(0);

        view.position.move_down(prepadding);
        simple_log.render(view);
        view.print()?;

        // Clear previous height, so this rendering does not get cleared.
        previous_height.take();
        render_info.previous_log_type.replace(LogType::Simple);
    }

    #[throws(Error)]
    fn print_progress_log(
        view: &mut RootView,
        render_info: &mut RenderInfo,
        progress_log: &ProgressLog,
        previous_height: &mut Option<usize>,
    ) {
        let is_finished = progress_log.is_finished();

        // Reset terminal cursor to the appropriate line determined by the previous height. The
        // previous height should only have been set if a running progress rendered last in the
        // last rendering pass.
        Self::reset_cursor(*previous_height)?;

        let prepadding = render_info
            .previous_log_type
            .map(|t| if t != LogType::RunningProgress { 2 } else { 0 })
            .unwrap_or(0);

        view.position.move_down(prepadding);
        progress_log.render(view, render_info.next_frame(), render_info.is_paused);

        let print_height = view.print()?;
        if is_finished {
            // Clear previous height, so this printing does not get cleared.
            previous_height.take();
            render_info
                .previous_log_type
                .replace(LogType::FinishedProgress);
        } else {
            previous_height.replace(print_height - prepadding);
            render_info
                .previous_log_type
                .replace(LogType::RunningProgress);
        }
    }

    #[throws(Error)]
    fn reset_cursor(previous_height: Option<usize>) {
        if let Some(height) = previous_height.filter(|h| *h > 1) {
            io::stdout().execute(cursor::MoveToPreviousLine(height as u16 - 1))?;
        } else {
            io::stdout().execute(cursor::MoveToColumn(0))?;
        }
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
            let _lock = is_paused_cvar
                .wait_while(is_paused_lock, |is_paused| *is_paused)
                .expect(EXPECT_THREAD_NOT_POSIONED);
        }
    }
}

struct RenderInfo {
    is_paused: bool,
    frames: animation::Frames,
    previous_log_type: Option<LogType>,
}

impl RenderInfo {
    fn new() -> Self {
        Self {
            is_paused: false,
            frames: animation::Frames::new(),
            previous_log_type: None,
        }
    }

    fn next_frame(&mut self) -> usize {
        self.frames
            .next()
            .expect("animation frames should be infinite")
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum LogType {
    Simple,
    RunningProgress,
    FinishedProgress,
}

impl SimpleLog {
    fn render_height(&self) -> usize {
        1
    }

    fn render(&self, view: &mut dyn View) {
        let (level_icon, color) = self.get_icon_and_color();

        view.set_color(color);
        view.render(&format!(
            "{level} {msg}",
            level = level_icon,
            msg = self.message
        ));
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

    fn render(&self, view: &mut dyn View, animation_frame: usize, is_paused: bool) {
        if view.max_height() == Some(0) {
            return;
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
        view.set_color(color);
        view.render(&format!(
            "{spin} {msg}",
            spin = animation::braille_spin(animation_state),
            msg = self.message,
        ));

        let render_elapsed = |view: &mut dyn View| match run_time {
            None => {
                let elapsed = self.start_time.elapsed();
                view.render(&format!(
                    "{secs}.{millis}s",
                    secs = elapsed.as_secs(),
                    millis = elapsed.as_millis() % 1000 / 100,
                ));
            }
            Some(elapsed) => view.render(&format!(
                "{secs}.{millis:03}s",
                secs = elapsed.as_secs(),
                millis = elapsed.as_millis() % 1000,
            )),
        };

        let render_no_nested = self.logs.is_empty() || view.max_height() == Some(1);

        if render_no_nested && is_paused && !is_finished {
            view.position_mut().move_down(1);
            view.position_mut().move_to_column(0);
            view.render(&format!(
                "{turn}{line}",
                turn = animation::box_turn_swell(animation_state),
                line = "╶".repeat(view.max_width() - 1)
            ));
            view.clear_color();

            return;
        }

        if render_no_nested {
            view.render(&format!(
                " {sep} ",
                sep = animation::separator_swell(animation_state)
            ));
            render_elapsed(view);
            view.clear_color();

            return;
        };

        // Reserve two rows for the header and the footer.
        let inner_max_height = view.max_height().map(|h| h - 2);
        // Keep track of the number of render lines required for the submessages.
        let mut remaining_height = self.render_height(is_paused) - 2;

        let start_row = view.position().row() + 1;
        let render_prefix = |view: &mut dyn View| {
            view.position_mut().move_down(1);
            view.position_mut().move_to_column(0);

            let frame_offset = -2 * (view.position().row() - start_row) as isize;

            view.render(&format!(
                "{side} ",
                side = animation::box_side_swell(animation_state.frame_offset(frame_offset)),
            ));
        };

        for log in self.logs.iter() {
            view.set_color(color);

            match log {
                Log::Simple(simple_log) => {
                    if inner_max_height.is_none() || Some(remaining_height - 1) < inner_max_height {
                        render_prefix(view);
                        simple_log.render(view);
                    }

                    remaining_height -= 1;
                }

                Log::Progress(progress_log) => {
                    let nested_height = progress_log.render_height(is_paused);

                    let max_height = match inner_max_height {
                        None => None,
                        Some(inner_max_height)
                            if remaining_height - nested_height < inner_max_height =>
                        {
                            Some(nested_height - remaining_height.saturating_sub(inner_max_height))
                        }
                        _ => Some(0),
                    };

                    let start_row = view.position().row() + 1;

                    // Print prefix.
                    for _ in 0..max_height.unwrap_or(nested_height) {
                        render_prefix(view);
                    }

                    let mut subview = view.subview(
                        Position::new(start_row, 2),
                        view.max_width() - 2,
                        max_height,
                    );
                    progress_log.render(&mut subview, animation_frame, is_paused);

                    remaining_height -= nested_height;
                }
            };
        }

        // Print prefix of elapsed line.
        view.set_color(color);
        view.position_mut().move_down(1);
        view.position_mut().move_to_column(0);
        view.render(
            &animation::box_turn_swell(
                animation_state.frame_offset(-2 * (view.position().row() - start_row) as isize),
            )
            .to_string(),
        );

        if is_paused && !is_finished {
            // Print dashed line to indicate paused, incomplete progress.
            view.render(&"╶".repeat(view.max_width() - 1));
        } else {
            // Print elapsed time.
            view.render(
                &animation::box_end_swell(
                    animation_state
                        .frame_offset(-2 * (view.position().row() - start_row + 1) as isize),
                )
                .to_string(),
            );
            render_elapsed(view);
        }

        view.clear_color();
    }
}
