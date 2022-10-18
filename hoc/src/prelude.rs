pub use anyhow::{bail, ensure, Context as AnyhowContext};
pub use fehler::{throw, throws};
pub use log::{debug, error, info, log, log_enabled, trace, warn, Level};

pub const EXPECT_CONTEXT_INITIALIZED: &'static str = "context is not initialized";
pub const EXPECT_RENDER_THREAD_INITIALIZED: &'static str = "render thread is not initialized";
pub const EXPECT_THREAD_NOT_POSIONED: &'static str = "thread should not be poisoned";
