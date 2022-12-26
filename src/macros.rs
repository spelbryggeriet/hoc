macro_rules! progress_with_handle {
    ($($args:tt)*) => {{
        $crate::log::progress(format!($($args)*))
    }};
}

macro_rules! progress {
    ($($args:tt)*) => {
        let _handle = progress_with_handle!($($args)*);
    };
}

macro_rules! prompt {
    ($($args:tt)*) => {{
        let msg = $crate::util::from_arguments_to_str_cow(format_args!($($args)*));
        $crate::prompt::PromptBuilder::new(msg)
    }};
}

macro_rules! select {
    ($($args:tt)*) => {{
        let msg = $crate::util::from_arguments_to_str_cow(format_args!($($args)*));
        $crate::prompt::SelectBuilder::new(msg)
    }};
}

macro_rules! kv {
    ($($args:tt)*) => {{
        let key = $crate::util::from_arguments_to_key_cow(format_args!($($args)*));
        $crate::context::KvBuilder::new(key)
    }};
}

macro_rules! files {
    ($($args:tt)*) => {{
        let key = $crate::util::from_arguments_to_key_cow(format_args!($($args)*));
        $crate::context::fs::FileBuilder::new(key)
    }};
}

macro_rules! temp_file {
    () => {
        $crate::context::Context::get_or_init().temp().create_file()
    };
}

macro_rules! process {
    ($($sudo:ident)? $fmt:literal $(<($stdin_fmt:literal))? $(, $arg_name:ident = $arg_val:expr)* $(,)?) => {{
        $(
        #[deny(unused)]
        let $arg_name = &$arg_val;
        )*

        let process = $crate::util::from_arguments_to_str_cow(format_args!($fmt));
        let mut builder = $crate::process::ProcessBuilder::new(process);

        if __is_sudo!($($sudo)?) {
            builder = builder.sudo();
        }

        $(
        builder = builder.write_stdin(&format!($stdin_fmt));
        )*

        builder
    }};
}

macro_rules! __is_sudo {
    (sudo) => {
        true
    };

    () => {
        false
    };

    ($token:tt) => {{
        compile_error!(concat!(
            "Expected `sudo` token, found `",
            stringify!($token),
            "`"
        ));
    }};
}
