use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use log_facade::Level;
use once_cell::sync::OnceCell;

use crate::{log::Error, prelude::*};
pub use drop_handle::DropHandle;
use render::PauseLock;

mod render;

pub fn init() {
    Progress::get_or_init();
    render::init();
}

#[throws(Error)]
pub fn cleanup() {
    render::cleanup()?;
}

#[throws(Error)]
pub fn pause_rendering(height: usize) -> PauseLock {
    let lock = render::RenderThread::pause(height)?;
    lock
}

fn last_running_subprogress_mut<'a>(
    logs: impl Iterator<Item = &'a mut Log>,
) -> Option<&'a mut ProgressLog> {
    logs.filter_map(|log| {
        if let Log::Progress(progress_log) = log {
            (!progress_log.is_finished()).then_some(progress_log)
        } else {
            None
        }
    })
    .last()
}

pub struct Progress {
    logs: Mutex<VecDeque<Log>>,
}

impl Progress {
    pub fn get_or_init() -> &'static Progress {
        static PROGRESS: OnceCell<Progress> = OnceCell::new();

        PROGRESS.get_or_init(Progress::new)
    }

    fn new() -> Self {
        Self {
            logs: Mutex::new(VecDeque::new()),
        }
    }

    pub fn push_simple_log(&self, level: Level, message: String) {
        // Find the current progress log.
        let mut logs_lock = self.logs.lock().expect(EXPECT_THREAD_NOT_POSIONED);
        let logs = &mut *logs_lock;
        let progress_log = last_running_subprogress_mut(logs.iter_mut());

        if let Some(progress_log) = progress_log {
            for line in message.lines() {
                progress_log.push_simple_log(SimpleLog::new(line.to_string()).with_level(level));
            }
        } else {
            for line in message.lines() {
                logs.push_back(Log::Simple(
                    SimpleLog::new(line.to_string()).with_level(level),
                ));
            }
        }
    }

    pub fn push_progress_log(&self, message: String) -> DropHandle {
        let (subprogress_log, drop_handle) = ProgressLog::new(message);

        // Find the current progress log.
        let mut logs_lock = self.logs.lock().expect(EXPECT_THREAD_NOT_POSIONED);
        let logs = &mut *logs_lock;
        let progress_log = last_running_subprogress_mut(logs.iter_mut());

        if let Some(progress_log) = progress_log {
            progress_log.push_progress_log(subprogress_log);
        } else {
            logs.push_back(Log::Progress(subprogress_log));
        }

        drop_handle
    }

    fn push_pause_log(
        &self,
        height: usize,
    ) -> (Arc<Mutex<bool>>, Arc<Mutex<Option<(Level, String)>>>) {
        let pause_log = PauseLog::new(height);
        let is_finished_mutex = Arc::clone(&pause_log.is_finished);
        let message_mutex = Arc::clone(&pause_log.message);

        // Find the current progress log.
        let mut logs_lock = self.logs.lock().expect(EXPECT_THREAD_NOT_POSIONED);
        let logs = &mut *logs_lock;
        let progress_log = last_running_subprogress_mut(logs.iter_mut());

        if let Some(progress_log) = progress_log {
            progress_log.push_pause_log(pause_log);
        } else {
            logs.push_back(Log::Pause(pause_log));
        }

        (is_finished_mutex, message_mutex)
    }

    fn logs(&self) -> MutexGuard<VecDeque<Log>> {
        self.logs.lock().expect(EXPECT_THREAD_NOT_POSIONED)
    }
}

#[derive(Debug)]
enum Log {
    Simple(SimpleLog),
    Progress(ProgressLog),
    Pause(PauseLog),
}

#[derive(Debug)]
struct SimpleLog {
    level: Option<Level>,
    message: String,
}

impl SimpleLog {
    fn new(message: String) -> Self {
        Self {
            level: None,
            message,
        }
    }

    fn with_level(mut self, level: Level) -> Self {
        self.level.replace(level);
        self
    }
}

#[derive(Debug)]
pub struct ProgressLog {
    message: String,
    start_time: Instant,
    logs: Vec<Log>,
    run_time: Arc<Mutex<Option<Duration>>>,
}

impl ProgressLog {
    fn is_finished(&self) -> bool {
        self.run_time
            .lock()
            .expect(EXPECT_THREAD_NOT_POSIONED)
            .is_some()
    }

    fn push_simple_log(&mut self, simple_log: SimpleLog) {
        if let Some(last_running_subprogress) = last_running_subprogress_mut(self.logs.iter_mut()) {
            last_running_subprogress.push_simple_log(simple_log);
        } else {
            self.logs.push(Log::Simple(simple_log));
        }
    }

    fn push_progress_log(&mut self, progress_log: ProgressLog) {
        if let Some(last_running_subprogress) = last_running_subprogress_mut(self.logs.iter_mut()) {
            last_running_subprogress.push_progress_log(progress_log);
        } else {
            self.logs.push(Log::Progress(progress_log));
        }
    }

    fn push_pause_log(&mut self, pause_log: PauseLog) {
        if let Some(last_running_subprogress) = last_running_subprogress_mut(self.logs.iter_mut()) {
            last_running_subprogress.push_pause_log(pause_log);
        } else {
            self.logs.push(Log::Pause(pause_log));
        }
    }
}

#[derive(Debug)]
struct PauseLog {
    height: usize,
    is_finished: Arc<Mutex<bool>>,
    message: Arc<Mutex<Option<(Level, String)>>>,
}

impl PauseLog {
    fn new(height: usize) -> Self {
        Self {
            height,
            is_finished: Arc::new(Mutex::new(false)),
            message: Arc::new(Mutex::new(None)),
        }
    }

    fn is_finished(&self) -> bool {
        *self.is_finished.lock().expect(EXPECT_THREAD_NOT_POSIONED)
    }
}

mod drop_handle {
    use super::*;

    impl ProgressLog {
        pub fn new(message: String) -> (Self, DropHandle) {
            let log = Self {
                message,
                start_time: Instant::now(),
                logs: Vec::new(),
                run_time: Arc::new(Mutex::new(None)),
            };
            let drop_handle = DropHandle::new(log.start_time, Arc::clone(&log.run_time));

            (log, drop_handle)
        }
    }

    #[must_use]
    pub struct DropHandle {
        start_time: Instant,
        run_time: Arc<Mutex<Option<Duration>>>,
    }

    impl DropHandle {
        fn new(start_time: Instant, run_time: Arc<Mutex<Option<Duration>>>) -> Self {
            Self {
                start_time,
                run_time,
            }
        }

        pub fn finish(self) {}
    }

    impl Drop for DropHandle {
        fn drop(&mut self) {
            self.run_time
                .lock()
                .expect(EXPECT_THREAD_NOT_POSIONED)
                .replace(self.start_time.elapsed());
        }
    }
}
