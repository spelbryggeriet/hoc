mod context;
mod log;
mod prefix;
mod styling;
mod wrapping;

use std::result::Result as StdResult;

use lazy_static::lazy_static;

pub use log::{Error, Log, LogErr, Status, Stream};
pub use styling::Styling;
pub use wrapping::Words;

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
    ($($fmt:tt)*) => {
        $crate::LOG.info(format!($($fmt)*))
    };
}

#[macro_export]
macro_rules! choose {
    ($($fmt:tt)*) => {
        $crate::LOG.choose(format!($($fmt)*))
    };
}

#[macro_export]
macro_rules! prompt {
    ($($fmt:tt)*) => {
        $crate::LOG.prompt(format!($($fmt)*))
    };
}

#[macro_export]
macro_rules! input {
    ($($fmt:tt)*) => {
        $crate::LOG.input(format!($($fmt)*))
    };
}

#[macro_export]
macro_rules! hidden_input {
    ($($fmt:tt)*) => {
        $crate::LOG.hidden_input(format!($($fmt)*))
    };
}

#[macro_export]
macro_rules! status {
    ($($fmt:tt)*) => {
        $crate::LOG.status(format!($($fmt)*))
    };
}

#[macro_export]
macro_rules! warning {
    ($($fmt:tt)*) => {
        $crate::LOG.warning(format!($($fmt)*))
    };
}

#[macro_export]
macro_rules! error {
    ($($fmt:tt)*) => {
        $crate::LOG.error(format!($($fmt)*))
    };
}

#[macro_export]
macro_rules! bail {
    ($($fmt:tt)*) => {
        return Err($crate::LOG.error(format!($($fmt)*)).unwrap_err().into());
    };
}

#[derive(Debug)]
pub enum Never {}

impl Never {
    pub fn into<T>(self) -> T {
        unreachable!()
    }
}

pub type Result<T> = StdResult<T, Error>;
