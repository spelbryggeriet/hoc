pub use anyhow::{bail, ensure, Context as AnyhowContext};
pub use fehler::{throw, throws};
pub use log_facade::{debug, error, info, log, warn, Level};

pub use crate::{
    context::kv::IteratorExt,
    log::{
        ProgressHandle, CLEAR_COLOR, DEBUG_COLOR, ERROR_COLOR, INFO_COLOR, TRACE_COLOR, WARN_COLOR,
    },
    util::Secret,
};

pub const EXPECT_HOME_ENV_VAR: &str = "HOME environment variable not set";
pub const EXPECT_THREAD_NOT_POSIONED: &str = "thread should not be poisoned";
