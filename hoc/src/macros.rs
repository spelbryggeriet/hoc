macro_rules! concat_const {
    ($first_segment:expr $(, $segments:expr)* $(,)?) => {{
        const __LEN: usize = $first_segment.len() $(+ $segments.len())*;

        const fn __copy_slice(
            input: &[u8],
            mut output: [u8; __LEN],
            offset: usize,
        ) -> (usize, [u8; __LEN]) {
            let mut index = 0;
            loop {
                output[offset + index] = input[index];
                index += 1;
                if index == input.len() {
                    break;
                }
            }
            (index + offset, output)
        }

        static mut __OUT: [u8; __LEN] = [0u8; __LEN];

        // SAFETY: `__OUT` is bound to the current scope, and is thus only accessible in the current thread.
        unsafe {
            let mut __offset = 0;
            (__offset, __OUT) = __copy_slice($first_segment.as_bytes(), __OUT, __offset);
            $(
            (__offset, __OUT) = __copy_slice($segments.as_bytes(), __OUT, __offset);
            )*
            ::std::str::from_utf8(&__OUT).unwrap_unchecked()
        }
    }};
}

macro_rules! args_summary {
    ($($field:ident(
        $(default = $default:literal,)?
        help = $help:literal
        $(, long_help = $long_help:literal)? $(,)?))+
    ) => {
        mod default {
            $(
            $(
            pub fn $field() -> &'static str {
                $default
            }
            )?
            )+
        }

        mod help {
            $(
            mod $field {
                pub const DEFAULT: (bool, &'static str) = {
                    #[allow(unused_variables)]
                    let val = (false, "");
                    $(
                    let val = (true, $default);
                    )?
                    val
                };
            }
            )+

            $(
            pub fn $field() -> &'static str {
                if $field::DEFAULT.0 {
                    concat_const!($help, " [default: ", $field::DEFAULT.1, "]")
                } else {
                    $help
                }
            }

            $(
            pub mod long {
                pub fn $field() -> &'static str {
                    if super::$field::DEFAULT.0 {
                        concat_const!(
                            $help,
                            "\n\n",
                            $long_help,
                            "\n\n[default: ",
                            super::$field::DEFAULT.1,
                            "]",
                        )
                    } else {
                        concat_const!($help, "\n\n", $long_help)
                    }
                }
            }
            )?
            )+
        }
    };
}

macro_rules! arg_get {
    ($self:ident, $field:ident $(,)?) => {
        $crate::prompt::Prompt::get($self.$field, stringify!($field))?
    };
}

macro_rules! arg_get_or_default {
    ($self:ident, $field:ident $(,)?) => {
        $crate::prompt::Prompt::get_or($self.$field, stringify!($field), default::$field())?
    };
}