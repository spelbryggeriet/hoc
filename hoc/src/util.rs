use std::{borrow::Cow, fmt::Arguments};

pub fn from_arguments_to_cow(arguments: Arguments) -> Cow<'static, str> {
    if let Some(s) = arguments.as_str() {
        Cow::Borrowed(s)
    } else {
        Cow::Owned(arguments.to_string())
    }
}
