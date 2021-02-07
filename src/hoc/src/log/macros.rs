#![macro_use]

macro_rules! _log {
    ($meta:tt ($($args:tt)*)) => {
        _log! { $meta ($($args)*) => () }
    };

    ($meta:tt ((), $($rest:tt)*) => ($($processed:tt)*)) => {
        _log! { $meta ($($rest)*) => ($($processed)* "",) }
    };

    ($meta:tt (($text:literal $(,)?), $($rest:tt)*) => ($($processed:tt)*)) => {
        _log! { $meta ($($rest)*) => ($($processed)* &$text,) }
    };

    ($meta:tt (($value:expr $(,)?), $($rest:tt)*) => ($($processed:tt)*)) => {
        _log! { $meta ($($rest)*) => ($($processed)* format!("{}", $value),) }
    };

    ($meta:tt (($($fmt:tt)*), $($rest:tt)*) => ($($processed:tt)*)) => {
        _log! { $meta ($($rest)*) => ($($processed)* format!($($fmt)*),) }
    };

    ([$method:ident] () => ($($processed:tt)*)) => {
        crate::LOG.$method($($processed)*)
    };
}

macro_rules! labelled_info {
    ($label:expr, $($args:tt)*) => {
        _log!([labelled_info] (($label), ($($args)*),))
    };
}

macro_rules! info {
    ($($args:tt)*) => {
        _log!([info] (($($args)*),))
    };
}

macro_rules! status {
    ($($args:tt)*) => {
        let _status = _log!([status] (($($args)*),));
    };
}

macro_rules! error {
    ($($args:tt)*) => {
        _log!([error] (($($args)*),))
    };
}

macro_rules! prompt {
    ($($args:tt)*) => {
        _log!([prompt] (($($args)*),))
    };
}

/// Ask for user input.
///
/// # Examples
///
/// ```rust
/// let name     = input!("Give me your name");
/// let password = input!([hidden] "Give me your password");
/// ```
macro_rules! input {
    ($($args:tt)*) => {
        _log!([input] (($($args)*),))
    };
}

macro_rules! hidden_input {
    ($($args:tt)*) => {
        _log!([hidden_input] (($($args)*),))
    };
}

macro_rules! choose {
    ($msg:expr, $items:expr $(, $default_index:expr)? $(,)?) => {
        crate::LOG.choose($msg, $items, $( if true { $default_index } else )? { 0 })
    };
}
