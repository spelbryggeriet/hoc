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

use self::view::{Position, RootView, View};
use super::{Log, LogType, PauseLog, ProgressLog, SimpleLog};
use crate::{log::Error, prelude::*};

#[macro_use]
mod view;

mod anim;
mod term;

pub fn init() {
    RenderThread::get_or_init();
}

#[throws(Error)]
pub fn cleanup() {
    if let Some(render_thread) = RenderThread::cell().get() {
        render_thread.terminate()?;
    }
}

#[must_use]
pub struct RenderThread {
    handle: Mutex<Option<JoinHandle<Result<(), Error>>>>,
    wants_terminate: Arc<AtomicBool>,
    wants_pause: Arc<(Mutex<Option<usize>>, Condvar)>,
    is_paused: Arc<(Mutex<Option<PauseData>>, Condvar)>,
}

impl RenderThread {
    fn get_or_init() -> &'static RenderThread {
        Self::cell().get_or_init(Self::new)
    }

    fn cell() -> &'static OnceCell<Self> {
        static RENDER_THREAD: OnceCell<RenderThread> = OnceCell::new();

        &RENDER_THREAD
    }

    fn new() -> Self {
        let wants_terminate_orig = Arc::new(AtomicBool::new(false));
        let wants_terminate = Arc::clone(&wants_terminate_orig);

        let wants_pause_orig = Arc::new((Mutex::new(None), Condvar::new()));
        let wants_pause = Arc::clone(&wants_pause_orig);

        let is_paused_orig = Arc::new((Mutex::new(None), Condvar::new()));
        let is_paused = Arc::clone(&is_paused_orig);

        let thread_handle = thread::spawn(move || {
            io::stdout().execute(cursor::Hide)?;

            let mut render_info = RenderInfo::new();
            let mut previous_height = None;

            let (terminal_cols, _) = terminal::size()?;
            let mut view = RootView::new(terminal_cols as usize);

            while !wants_terminate.load(Ordering::SeqCst) {
                let (terminal_cols, terminal_rows) = terminal::size()?;

                view.set_max_width(terminal_cols as usize);

                {
                    let (wants_pause_mutex, wants_pause_cvar) = &*wants_pause;
                    let wants_pause_lock =
                        wants_pause_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);

                    if let Some(pause_height) = *wants_pause_lock {
                        render_info.is_paused = true;

                        let progress = super::Progress::get_or_init();
                        if progress.last_log_type() == Some(LogType::RunningProgress) {
                            progress.push_empty_log();
                        }
                        let (is_finished_mutex, message_mutex) =
                            progress.push_pause_log(pause_height);

                        let mut logs_mutex = progress.logs();
                        while let Some(log) = logs_mutex.pop_front() {
                            view.set_infinite_height();

                            match &log {
                                Log::Simple(simple_log) => Self::print_simple_log(
                                    &mut view,
                                    &mut render_info,
                                    simple_log,
                                    &mut previous_height,
                                )?,

                                Log::Progress(progress_log) => {
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
                                        logs_mutex.push_front(log);
                                        break;
                                    }
                                }

                                Log::Pause(pause_log) => {
                                    Self::print_pause_log(
                                        &mut view,
                                        &mut render_info,
                                        pause_log,
                                        &mut previous_height,
                                    )?;
                                    logs_mutex.push_front(log);
                                    break;
                                }
                            }
                        }
                        drop(logs_mutex);

                        if render_info.previous_log_type == Some(LogType::RunningProgress) {
                            progress.push_empty_log();
                        }

                        let pause_cursor = render_info
                            .pause_cursor
                            .expect("pause curser should be set");

                        // Calculate the amount of lines to get to the start of the pause area.
                        let line_diff = if let Some(previous_height) = previous_height {
                            previous_height - pause_cursor.row() - 1
                        } else {
                            pause_height.saturating_sub(1)
                        };

                        term::move_cursor_up(line_diff as u16)?;

                        {
                            let (is_paused_mutex, is_paused_cvar) = &*is_paused;
                            let mut is_paused_lock =
                                is_paused_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);
                            is_paused_lock.replace(PauseData {
                                message: message_mutex,
                                indentation: pause_cursor.column(),
                            });
                            is_paused_cvar.notify_one();
                        }

                        {
                            let _lock = wants_pause_cvar
                                .wait_while(wants_pause_lock, |wants_pause| wants_pause.is_some())
                                .expect(EXPECT_THREAD_NOT_POSIONED);
                        }

                        if previous_height.is_some() {
                            let line_diff = line_diff - pause_height.saturating_sub(1);
                            term::move_cursor_down(line_diff as u16)?;
                            term::move_cursor_to_column(0)?;
                        } else {
                            term::clear(pause_height as u16)?;
                        }

                        let mut is_finished_lock =
                            is_finished_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);
                        *is_finished_lock = true;

                        {
                            let (is_paused_mutex, is_paused_cvar) = &*is_paused;
                            let mut is_paused_lock =
                                is_paused_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);
                            is_paused_lock.take();
                            is_paused_cvar.notify_one();
                        }
                    }

                    {
                        view.set_max_width(terminal_cols as usize);

                        render_info.is_paused = false;

                        let mut logs = super::Progress::get_or_init().logs();
                        while let Some(log) = logs.pop_front() {
                            view.set_infinite_height();

                            match &log {
                                Log::Simple(simple_log) => Self::print_simple_log(
                                    &mut view,
                                    &mut render_info,
                                    simple_log,
                                    &mut previous_height,
                                )?,

                                Log::Progress(progress_log) => {
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

                                Log::Pause(pause_log) => Self::print_pause_log(
                                    &mut view,
                                    &mut render_info,
                                    pause_log,
                                    &mut previous_height,
                                )?,
                            }
                        }
                    }
                }

                spin_sleep::sleep(Duration::from_nanos(16_666_667));

                render_info.advance_animation();
            }

            let (terminal_cols, _) = terminal::size()?;

            view.set_max_width(terminal_cols as usize);
            view.set_infinite_height();

            let mut logs = super::Progress::get_or_init().logs();
            for log in logs.drain(..) {
                match &log {
                    Log::Simple(simple_log) => Self::print_simple_log(
                        &mut view,
                        &mut render_info,
                        simple_log,
                        &mut previous_height,
                    )?,

                    Log::Progress(progress_log) if progress_log.is_finished() => {
                        Self::print_progress_log(
                            &mut view,
                            &mut render_info,
                            progress_log,
                            &mut previous_height,
                        )?;
                    }

                    Log::Pause(pause_log) => Self::print_pause_log(
                        &mut view,
                        &mut render_info,
                        pause_log,
                        &mut previous_height,
                    )?,

                    Log::Progress(_) => panic!("no progress should be running after cancellation"),
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
    pub fn pause(height: usize) -> PauseLock {
        PauseLock::new(height)?
    }

    #[throws(Error)]
    fn print_simple_log(
        view: &mut RootView,
        render_info: &mut RenderInfo,
        simple_log: &SimpleLog,
        previous_height: &mut Option<usize>,
    ) {
        let prepadding = render_info.previous_log_type.map(|t| {
            if t != LogType::Simple && t != LogType::Pause {
                2
            } else {
                1
            }
        });

        if let Some(prepadding) = prepadding {
            view.cursor_mut().move_down(prepadding);
            render!(view => "");
            view.print()?;
        }

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
        if let Some(previous_height) = previous_height {
            term::move_cursor_up(previous_height.saturating_sub(1) as u16)?;
        }
        term::move_cursor_to_column(0)?;

        let prepadding = render_info
            .previous_log_type
            .and_then(|t| (t != LogType::RunningProgress).then_some(2));

        if let Some(prepadding) = prepadding {
            view.cursor_mut().move_down(prepadding);
            render!(view => "");
            view.print()?;
        }

        progress_log.render(view, render_info);
        let print_height = view.print()?;

        let previous_height = if is_finished {
            render_info
                .previous_log_type
                .replace(LogType::FinishedProgress);
            previous_height.take()
        } else {
            render_info
                .previous_log_type
                .replace(LogType::RunningProgress);
            previous_height.replace(print_height)
        };

        if let Some(previous_height) = previous_height {
            if previous_height > print_height {
                let diff = previous_height as u16 - print_height as u16;
                term::move_cursor_down(diff)?;
                term::clear(diff)?;
            }
        }
    }

    #[throws(Error)]
    fn print_pause_log(
        view: &mut RootView,
        render_info: &mut RenderInfo,
        pause_log: &PauseLog,
        previous_height: &mut Option<usize>,
    ) {
        let prepadding = render_info.previous_log_type.and_then(|_| {
            if pause_log.height > 0 && render_info.is_paused
                || pause_log
                    .message
                    .lock()
                    .expect(EXPECT_THREAD_NOT_POSIONED)
                    .is_some()
            {
                Some(1)
            } else {
                None
            }
        });

        if let Some(prepadding) = prepadding {
            view.cursor_mut().move_down(prepadding);
            render!(view => "");
            view.print()?;
        }

        pause_log.render(view, render_info);
        view.print()?;

        // Clear previous height, so this rendering does not get cleared.
        previous_height.take();
        render_info.previous_log_type.replace(LogType::Pause);
    }

    #[throws(Error)]
    #[allow(unused)]
    fn terminal_reset_cursor(previous_height: Option<usize>) {
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
pub struct PauseLock {
    data: PauseData,
}

impl PauseLock {
    #[throws(Error)]
    fn new(height: usize) -> Self {
        let render_thread = RenderThread::get_or_init();

        {
            let (wants_pause_mutex, wants_pause_cvar) = &*render_thread.wants_pause;
            let mut wants_pause_lock = wants_pause_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);

            if wants_pause_lock.is_some() {
                throw!(Error::PauseLockAlreadyAcquired);
            }

            wants_pause_lock.replace(height);
            wants_pause_cvar.notify_one();
        }

        let data = {
            let (is_paused_mutex, is_paused_cvar) = &*render_thread.is_paused;
            let mut is_paused_lock = is_paused_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);
            loop {
                is_paused_lock = match &mut *is_paused_lock {
                    None => is_paused_cvar
                        .wait(is_paused_lock)
                        .expect(EXPECT_THREAD_NOT_POSIONED),
                    Some(data) => {
                        break PauseData {
                            message: Arc::clone(&data.message),
                            indentation: data.indentation,
                        }
                    }
                }
            }
        };

        Self { data }
    }

    pub fn indentation(&self) -> usize {
        self.data.indentation
    }

    pub fn finish_with_message(self, level: Level, message: String) {
        self.data
            .message
            .lock()
            .expect(EXPECT_THREAD_NOT_POSIONED)
            .replace((level, message));
    }
}

impl Drop for PauseLock {
    fn drop(&mut self) {
        let render_thread = RenderThread::get_or_init();

        {
            let (wants_pause_mutex, wants_pause_cvar) = &*render_thread.wants_pause;
            let mut wants_pause_lock = wants_pause_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);
            wants_pause_lock.take();
            wants_pause_cvar.notify_one();
        }

        {
            let (is_paused_mutex, is_paused_cvar) = &*render_thread.is_paused;
            let is_paused_lock = is_paused_mutex.lock().expect(EXPECT_THREAD_NOT_POSIONED);
            let _lock = is_paused_cvar
                .wait_while(is_paused_lock, |is_paused| is_paused.is_some())
                .expect(EXPECT_THREAD_NOT_POSIONED);
        }
    }
}

struct PauseData {
    message: Arc<Mutex<Option<(Level, String)>>>,
    indentation: usize,
}

struct RenderInfo {
    is_paused: bool,
    pause_cursor: Option<Position>,
    frames: anim::Frames,
    animation_frame: usize,
    previous_log_type: Option<LogType>,
}

impl RenderInfo {
    const EXPECT_INFINITE_ANIM: &str = "animation frames should be infinite";

    fn new() -> Self {
        let mut frames = anim::Frames::new();
        let animation_frame = frames.next().expect(Self::EXPECT_INFINITE_ANIM);
        Self {
            is_paused: false,
            pause_cursor: None,
            frames,
            animation_frame,
            previous_log_type: None,
        }
    }

    fn advance_animation(&mut self) {
        self.animation_frame = self.frames.next().expect(Self::EXPECT_INFINITE_ANIM);
    }
}

impl SimpleLog {
    fn render_height(&self) -> usize {
        1
    }

    fn render(&self, view: &mut dyn View) {
        if let Some((level_icon, color)) = self.get_icon_and_color() {
            view.set_color(color);
            render!(view =>
                level_icon,
                " ",
                self.message,
            );
            view.clear_color();
        } else {
            view.clear_color();
            render!(view => self.message);
        }
    }

    #[throws(as Option)]
    fn get_icon_and_color(&self) -> (char, style::Color) {
        match self.level? {
            Level::Error => ('\u{f00d}', style::Color::Red),
            Level::Warn => ('\u{f12a}', style::Color::Yellow),
            Level::Info => ('\u{f48b}', style::Color::White),
            Level::Debug => ('\u{fd2b}', style::Color::DarkMagenta),
            Level::Trace => ('\u{e241}', style::Color::DarkGrey),
        }
    }
}

impl ProgressLog {
    const RUNNING_COLOR: style::Color = style::Color::Yellow;
    const FINISHED_COLOR: style::Color = style::Color::DarkCyan;

    fn render_height(&self, render_info: &RenderInfo) -> usize {
        if self.logs.is_empty() && !render_info.is_paused {
            1
        } else {
            2 + self
                .logs
                .iter()
                .map(|log| match log {
                    Log::Simple(simple_log) => simple_log.render_height(),
                    Log::Progress(progress_log) => progress_log.render_height(render_info),
                    Log::Pause(pause_log) => pause_log.render_height(),
                })
                .sum::<usize>()
        }
    }

    fn render(&self, view: &mut impl View, render_info: &mut RenderInfo) {
        if view.max_height() == Some(0) {
            return;
        }

        let run_time = *self.run_time.lock().expect(EXPECT_THREAD_NOT_POSIONED);
        let is_finished = run_time.is_some();

        let animation_state = if is_finished {
            anim::State::Finished
        } else if render_info.is_paused {
            anim::State::Paused
        } else {
            anim::State::Animating(render_info.animation_frame)
        };
        let color = if is_finished {
            Self::FINISHED_COLOR
        } else {
            Self::RUNNING_COLOR
        };

        // Print indicator and progress message.
        view.set_color(color);
        render!(view =>
            anim::braille_spin(animation_state),
            " ",
            self.message,
        );

        let render_elapsed = |view: &mut _| match run_time {
            None => {
                let elapsed = self.start_time.elapsed();
                render!(view =>
                    elapsed.as_secs(),
                    ".",
                    elapsed.as_millis() % 1000 / 100,
                    "s",
                );
            }
            Some(elapsed) => {
                render!(view =>
                    elapsed.as_secs(),
                    ".",
                    format!("{:03}", elapsed.as_millis() % 1000),
                    "s",
                );
            }
        };

        let render_no_nested = self.logs.is_empty() || view.max_height() == Some(1);

        if render_no_nested && render_info.is_paused && !is_finished {
            view.cursor_mut().move_down(1);
            view.cursor_mut().move_to_column(0);
            render!(view =>
                anim::box_turn_swell(animation_state),
                "╶".repeat(view.max_width() - 1),
            );
            view.clear_color();

            return;
        }

        if render_no_nested {
            render!(view =>
                " ",
                anim::separator_swell(animation_state),
                " ",
            );
            render_elapsed(view);
            view.clear_color();

            return;
        };

        // Reserve two rows for the header and the footer.
        let inner_max_height = view.max_height().map(|h| h - 2);
        // Keep track of the number of render lines required for the submessages.
        let mut remaining_height = self.render_height(render_info) - 2;

        let start_row = view.cursor().row() + 1;
        let render_prefix = |view: &mut _| {
            View::cursor_mut(view).move_down(1);
            View::cursor_mut(view).move_to_column(0);

            let frame_offset = -2 * (View::cursor(view).row() - start_row) as isize;

            render!(view =>
                anim::box_side_swell(animation_state.frame_offset(frame_offset)),
                " ",
            );
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
                    let nested_height = progress_log.render_height(render_info);

                    let max_height = match inner_max_height {
                        None => None,
                        Some(inner_max_height)
                            if remaining_height - nested_height < inner_max_height =>
                        {
                            Some(nested_height - remaining_height.saturating_sub(inner_max_height))
                        }
                        _ => Some(0),
                    };

                    let start_row = view.cursor().row() + 1;

                    // Print prefix.
                    for _ in 0..max_height.unwrap_or(nested_height) {
                        render_prefix(view);
                    }

                    let mut subview = view.subview(
                        Position::new(start_row, 2),
                        view.max_width() - 2,
                        max_height,
                    );
                    progress_log.render(&mut subview, render_info);

                    remaining_height -= nested_height;
                }

                Log::Pause(pause_log) => {
                    let nested_height = pause_log.render_height();

                    let max_height = match inner_max_height {
                        None => None,
                        Some(inner_max_height)
                            if remaining_height - nested_height < inner_max_height =>
                        {
                            Some(nested_height - remaining_height.saturating_sub(inner_max_height))
                        }
                        _ => Some(0),
                    };

                    let start_row = view.cursor().row() + 1;

                    // Print prefix.
                    for _ in 0..max_height.unwrap_or(nested_height) {
                        render_prefix(view);
                    }

                    let mut subview = view.subview(
                        Position::new(start_row, 2),
                        view.max_width() - 2,
                        max_height,
                    );
                    pause_log.render(&mut subview, render_info);

                    remaining_height -= nested_height;
                }
            };
        }

        // Print prefix of elapsed line.
        view.set_color(color);
        view.cursor_mut().move_down(1);
        view.cursor_mut().move_to_column(0);
        render!(view =>
            anim::box_turn_swell(
                animation_state.frame_offset(-2 * (view.cursor().row() - start_row) as isize),
            ),
        );

        if render_info.is_paused && !is_finished {
            // Print dashed line to indicate paused, incomplete progress.
            render!(view =>
                "╶".repeat(view.max_width() - 1),
            );
        } else {
            // Print elapsed time.
            render!(view =>
                anim::box_end_swell(
                    animation_state
                        .frame_offset(-2 * (view.cursor().row() - start_row + 1) as isize),
                ),
            );
            render_elapsed(view);
        }

        view.clear_color();
    }
}

impl PauseLog {
    fn render_height(&self) -> usize {
        if !self.is_finished() {
            self.height
        } else {
            self.message
                .lock()
                .expect(EXPECT_THREAD_NOT_POSIONED)
                .as_ref()
                .map_or(0, |_| 1)
        }
    }

    fn render(&self, view: &mut dyn View, render_info: &mut RenderInfo) {
        if !self.is_finished() {
            render_info.pause_cursor.replace(view.real_cursor());

            let height = if let Some(max_height) = view.max_height() {
                max_height.min(self.height)
            } else {
                self.height
            };

            if height > 0 {
                view.cursor_mut().move_down(height - 1);
                view.cursor_mut().move_to_column(0);
                render!(view => "");
            }
        } else if let Some((level, message)) =
            &*self.message.lock().expect(EXPECT_THREAD_NOT_POSIONED)
        {
            render_info.pause_cursor.take();
            SimpleLog::new(message.clone())
                .with_level(*level)
                .render(view);
        }
    }
}
