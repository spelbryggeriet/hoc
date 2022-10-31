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
pub fn pause_rendering() -> PauseLock {
    render::RenderThread::pause()?
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

    pub fn push_simple_log(&self, level: Level, message: String) {
        // Find the current progress log.
        let mut logs_lock = self.logs.lock().expect(EXPECT_THREAD_NOT_POSIONED);
        let logs = &mut *logs_lock;
        let progress_log = last_running_subprogress_mut(logs.iter_mut());

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

    fn logs(&self) -> MutexGuard<VecDeque<Log>> {
        self.logs.lock().expect(EXPECT_THREAD_NOT_POSIONED)
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
