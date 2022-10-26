macro_rules! progress {
    ($($args:tt)*) => {{
        $crate::logger::progress(format!($($args)*))
    }};
}

macro_rules! progress_scoped {
    ($($args:tt)*) => {
        let __progress = progress!($($args)*);
    };
}

macro_rules! prompt {
    ($($args:tt)*) => {{
        let __cow = $crate::util::from_arguments_to_str_cow(format_args!($($args)*));
        $crate::prompt::PromptBuilder::new(__cow)
    }};
}

macro_rules! select {
    ($($args:tt)*) => {{
        let __cow = $crate::util::from_arguments_to_str_cow(format_args!($($args)*));
        $crate::prompt::SelectBuilder::new(__cow)
    }};
}

macro_rules! put {
    ($value:expr => $($args:tt)*) => {{
        let __cow = $crate::util::try_from_arguments_to_key_cow(format_args!($($args)*))?;
        $crate::context::CONTEXT
            .get()
            .expect($crate::prelude::EXPECT_CONTEXT_INITIALIZED)
            .kv_put_value(
                __cow,
                $value,
            )
    }};
}

macro_rules! context_file {
    ($($args:tt)*) => {{
        let __cow = $crate::util::try_from_arguments_to_key_cow(format_args!($($args)*))?;
        $crate::context::FileBuilder::new(__cow)
    }};
}
