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

macro_rules! _temp_file {
    () => {
        async {
            $crate::context::get_context()
                .temp_mut()
                .await
                .create_file()
        }
    };
}

macro_rules! cmd {
    ($($sudo:ident)? $fmt:literal $($args:tt)*) => {{
        let is_sudo = $(if __is_sudo!($sudo) {
            true
        } else)? {
            false
        };

        let cmd = $crate::util::from_arguments_to_str_cow(format_args!($fmt $($args)*));
        let mut builder = $crate::runner::CmdBuilder::new(cmd);

        if is_sudo {
            builder = builder.sudo();
        }

        builder
    }};
}

macro_rules! __is_sudo {
    (sudo) => {
        true
    };

    ($token:tt) => {{
        compile_error!(concat!(
            "Expected `sudo` token, found `",
            stringify!($token),
            "`"
        ));
    }};
}
