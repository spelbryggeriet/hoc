pub use anyhow::{bail, ensure, Context as AnyhowContext};
pub use fehler::{throw, throws};
pub use log_facade::{debug, error, info, log, log_enabled, trace, warn, Level};

pub use crate::{context::kv::IteratorExt, util::Secret};

pub const EXPECT_THREAD_NOT_POSIONED: &str = "thread should not be poisoned";