mod context;
mod log;
mod prefix;
mod styling;
mod wrapping;

use std::result::Result as StdResult;

use lazy_static::lazy_static;

pub use log::{Error, Log, LogErr, Status, Stream};
pub use styling::Styling;
pub use wrapping::Wrapping;

lazy_static! {
    pub static ref LOG: Log = {
        let current_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            LOG.set_failure();
            current_hook(info);
        }));
        Log::new()
    };
}

#[macro_export]
macro_rules! info {
    ($fmt:expr $(,)?) => {
        $crate::LOG.info($fmt)
    };

    ($($fmt:tt)*) => {
        $crate::LOG.info(format!($($fmt)*))
    };
}

#[macro_export]
macro_rules! choose {
    (($($fmt:tt)*), items=$items:expr $(, default_index=$default_index:expr)? $(,)?) => {
        $crate::LOG.choose(format!($($fmt)*), $items, $( if true { $default_index } else )? { 0 })
    };

    ($fmt:expr, items=$items:expr $(, default_index=$default_index:expr)? $(,)?) => {
        $crate::LOG.choose($fmt, $items, $( if true { $default_index } else )? { 0 })
    };
}

#[macro_export]
macro_rules! status {
    (($($fmt:tt)*) $(, $code:expr)? $(,)?) => {{
        let __status = $crate::LOG.status(format!($($fmt)*));
        $($code)?
    }};

    ($fmt:expr $(, $code:expr)? $(,)?) => {{
        let __status = $crate::LOG.status($fmt);
        $($code)?
    }};
}

#[macro_export]
macro_rules! warning {
    ($fmt:expr $(,)?) => {
        $crate::LOG.warning($fmt)
    };

    ($($fmt:tt)*) => {
        $crate::LOG.warning(format!($($fmt)*))
    };
}

#[macro_export]
macro_rules! error {
    ($fmt:expr $(,)?) => {
        $crate::LOG.error($fmt)
    };

    ($($fmt:tt)*) => {
        $crate::LOG.error(format!($($fmt)*))
    };
}

#[macro_export]
macro_rules! bail {
    ($fmt:expr $(,)?) => {
        return Err($crate::LOG.error($fmt).unwrap_err().into());
    };

    ($($fmt:tt)*) => {
        return Err($crate::LOG.error(format!($($fmt)*)).unwrap_err().into());
    };
}

#[derive(Debug)]
pub enum Never {}

pub type Result<T> = StdResult<T, Error>;
