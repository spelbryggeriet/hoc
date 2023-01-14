macro_rules! progress_with_handle {
    ($level:ident, $($args:tt)*) => {{
        $crate::log::progress(format!($($args)*), Some($crate::prelude::Level::$level), module_path!())
    }};

    ($($args:tt)*) => {{
        $crate::log::progress(format!($($args)*), None, module_path!())
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

/// ## Example
///
/// ```no_run
/// let captured = "one";
/// let payload = 10;
/// process!(ENV_VAR="value_{captured}" "command {arg}" <("stdin data: {payload}"), arg = 1 + 1);
/// process!(ENV_VAR="value_{captured}" sudo "command {arg}" <("stdin data: {payload}"), arg = 1 + 1);
/// ```
macro_rules! process {
    (@impl ($fmt:literal $($rest:tt)*) => {
        fmt: [],
        args: [],
        stdin_data: [],
        env: [$($env:tt)*],
        sudo: $sudo:tt,
        prefix_env_vars: $prefix_env_vars:tt,
    }) => {{
        process!(@impl ($($rest)*) => {
            fmt: [$fmt],
            args: [],
            stdin_data: [],
            env: [$($env)*],
            sudo: $sudo,
            prefix_env_vars: $prefix_env_vars,
        })
    }};

    (@impl (sudo $($rest:tt)*) => {
        fmt: [],
        args: [],
        stdin_data: [],
        env: [$($env:tt)*],
        sudo: false,
        prefix_env_vars: $prefix_env_vars:tt,
    }) => {{
        process!(@impl ($($rest)*) => {
            fmt: [],
            args: [],
            stdin_data: [],
            env: [$($env)*],
            sudo: true,
            prefix_env_vars: $prefix_env_vars,
        })
    }};

    (@impl ($env_name:ident=$env_value:literal $($rest:tt)*) => {
        fmt: [],
        args: [],
        stdin_data: [],
        env: [$($env:tt)*],
        sudo: false,
        prefix_env_vars: $prefix_env_vars:tt,
    }) => {{
        process!(@impl ($($rest)*) => {
            fmt: [],
            args: [],
            stdin_data: [],
            env: [$($env)* $env_name=$env_value,],
            sudo: false,
            prefix_env_vars: true,
        })
    }};

    (@impl ($env_name:ident $($rest:tt)*) => {
        fmt: [],
        args: [],
        stdin_data: [],
        env: [$($env:tt)*],
        sudo: false,
        prefix_env_vars: $prefix_env_vars:tt,
    }) => {{
        process!(@impl ($($rest)*) => {
            fmt: [],
            args: [],
            stdin_data: [],
            env: [$($env)* $env_name,],
            sudo: false,
            prefix_env_vars: $prefix_env_vars,
        })
    }};

    (@impl (<($stdin_data:literal) $($rest:tt)*) => {
        fmt: [$fmt:tt],
        args: [],
        stdin_data: [],
        env: [$($env:tt)*],
        sudo: $sudo:tt,
        prefix_env_vars: $prefix_env_vars:tt,
    }) => {{
        process!(@impl ($($rest)*) => {
            fmt: [$fmt],
            args: [],
            stdin_data: [$stdin_data],
            env: [$($env)*],
            sudo: $sudo,
            prefix_env_vars: $prefix_env_vars,
        })
    }};

    (@impl ($(, $arg_name:ident = $arg_value:expr)+ $(,)?) => {
        fmt: [$fmt:tt],
        args: [],
        stdin_data: [$($stdin_data:tt)?],
        env: [$($env:tt)*],
        sudo: $sudo:tt,
        prefix_env_vars: $prefix_env_vars:tt,
    }) => {{
        process!(@impl () => {
            fmt: [$fmt],
            args: [$($arg_name = $arg_value,)+],
            stdin_data: [$($stdin_data)?],
            env: [$($env)*],
            sudo: $sudo,
            prefix_env_vars: $prefix_env_vars,
        })
    }};

    (@impl ($(,)?) => {
        fmt: [$fmt:literal],
        args: [$($arg_name:ident = $arg_value:expr,)*],
        stdin_data: [$($stdin_data:literal)?],
        env: [$($env_name:ident$(=$env_value:literal)?,)*],
        sudo: $sudo:literal,
        prefix_env_vars: $prefix_env_vars:literal,
    }) => {{
        $(
        #[deny(unused)]
        let $arg_name = &$arg_value;
        )*

        let process = $crate::util::from_arguments_to_str_cow(format_args!($fmt));
        let mut builder = $crate::process::ProcessBuilder::new(process);

        $(
        let env_value: Option<::std::borrow::Cow<'static, str>> = $(if true {
            Some($crate::util::from_arguments_to_str_cow(format_args!($env_value)))
        } else )? {
            None
        };
        builder = builder.env_var(stringify!($env_name), env_value);
        )*

        if $sudo {
            builder = builder.sudo();
        }

        if $prefix_env_vars {
            builder = builder.prefix_env_vars();
        }

        $(
        builder = builder.write_stdin(&format!($stdin_data));
        )?

        builder
    }};

    (@impl ($unexpected:tt $($rest:tt)*) => { $($irrelevant:tt)* }) => {{
        compile_error!(concat!("unexpected input: ", stringify!($unexpected)))
    }};

    ($($args:tt)*) => {{
        process!(@impl ($($args)*) => {
            fmt: [],
            args: [],
            stdin_data: [],
            env: [],
            sudo: false,
            prefix_env_vars: false,
        })
    }};
}

#[allow(unused)]
macro_rules! shell {
    () => {{
        $crate::process::Shell::new()
    }};
}
