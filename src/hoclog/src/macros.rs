#![macro_use]

#[macro_export]
macro_rules! _log {
    ($meta:tt ($($args:tt)*)) => {
        $crate::_log! { $meta ($($args)*) => () }
    };

    ($meta:tt ((), $($rest:tt)*) => ($($processed:tt)*)) => {
        $crate::_log! { $meta ($($rest)*) => ($($processed)* "",) }
    };

    ($meta:tt (($text:literal $(,)?), $($rest:tt)*) => ($($processed:tt)*)) => {
        $crate::_log! { $meta ($($rest)*) => ($($processed)* &$text,) }
    };

    ($meta:tt (($value:expr $(,)?), $($rest:tt)*) => ($($processed:tt)*)) => {
        $crate::_log! { $meta ($($rest)*) => ($($processed)* format!("{}", $value),) }
    };

    ($meta:tt (($($fmt:tt)*), $($rest:tt)*) => ($($processed:tt)*)) => {
        $crate::_log! { $meta ($($rest)*) => ($($processed)* format!($($fmt)*),) }
    };

    ([$method:ident] () => ($($processed:tt)*)) => {
        crate::LOG.$method($($processed)*)
    };
}

#[macro_export]
macro_rules! labelled_info {
    ($label:expr, $($args:tt)*) => {
        $crate::_log!([labelled_info] (($label), ($($args)*),))
    };
}

#[macro_export]
macro_rules! info {
    ($($args:tt)*) => {
        $crate::_log!([info] (($($args)*),))
    };
}

#[macro_export]
macro_rules! status {
    ($($args:tt)*) => {
        let _status = $crate::_log!([status] (($($args)*),));
    };
}

#[macro_export]
macro_rules! error {
    ($($args:tt)*) => {
        $crate::_log!([error] (($($args)*),))
    };
}

#[macro_export]
macro_rules! prompt {
    ($($args:tt)*) => {
        $crate::_log!([prompt] (($($args)*),))
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
#[macro_export]
macro_rules! input {
    ($($args:tt)*) => {
        $crate::_log!([input] (($($args)*),))
    };
}

#[macro_export]
macro_rules! hidden_input {
    ($($args:tt)*) => {
        $crate::_log!([hidden_input] (($($args)*),))
    };
}

#[macro_export]
macro_rules! choose {
    ($msg:expr, $items:expr $(, $default_index:expr)? $(,)?) => {
        crate::LOG.choose($msg, $items, $( if true { $default_index } else )? { 0 })
    };
}
