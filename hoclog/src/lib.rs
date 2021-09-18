mod styling;
mod wrapping;

use lazy_static::lazy_static;

pub use log::{Log, LogError};
pub use styling::Styling;
pub use wrapping::Wrapping;

mod context;
mod log;
mod prefix;
mod status;
mod stream;

lazy_static! {
    pub static ref LOG: Log = Log::new();
}

#[macro_export]
macro_rules! info {
    ($fmt:expr) => {
        $crate::LOG.info($fmt)
    };

    ($($fmt:tt)*) => {
        info!(format!($($fmt)*))
    };
}

#[macro_export]
macro_rules! status {
    (($($fmt:tt)*) $($rest:tt)*) =>  {
        status!(format!($($fmt)*) $($rest)*)
    };

    ($fmt:expr => $code:expr) => {{
        let __status = $crate::LOG.status($fmt);
        $code
    }};
}

#[macro_export]
macro_rules! warning {
    ($fmt:expr) => {
        $crate::LOG.warning($fmt)
    };

    ($($fmt:tt)*) => {
        warning!(format!($($fmt)*))
    };
}

#[macro_export]
macro_rules! error {
    ($fmt:expr) => {
        $crate::LOG.error($fmt)
    };

    ($($fmt:tt)*) => {
        error!(format!($($fmt)*))
    };
}

pub enum Never {}
