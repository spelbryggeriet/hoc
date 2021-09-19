mod context;
mod log;
mod prefix;
mod styling;
mod wrapping;

use std::result::Result as StdResult;

use lazy_static::lazy_static;

pub use log::{Error, Log, Status, Stream};
pub use styling::Styling;
pub use wrapping::Wrapping;

lazy_static! {
    pub static ref LOG: Log = Log::new();
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
macro_rules! status {
    (($($fmt:tt)*) $(, label=$label:expr)? $(=> $code:expr)? $(,)?) => {{
        let __status = $crate::LOG.status(format!($($fmt)*))
            $(.with_label($label))?;
        $($code)?
    }};

    ($fmt:expr $(, label=$label:expr)? $(=> $code:expr)? $(,)?) => {{
        let __status = $crate::LOG.status($fmt);
            $(.with_label($label))?;
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

pub enum Never {}

pub type Result<T> = StdResult<T, Error>;
