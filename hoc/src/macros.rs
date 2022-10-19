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
        $crate::prompt::PromptBuilder::new(
            $crate::util::from_arguments_to_cow(format_args!($($args)*)),
        )
    }};
}

macro_rules! select {
    ($($args:tt)*) => {{
        $crate::prompt::select(format!($($args)*))
    }};
}

macro_rules! put {
    ($value:expr => $($args:tt)*) => {
        $crate::context::CONTEXT
            .get()
            .expect($crate::prelude::EXPECT_CONTEXT_INITIALIZED)
            .kv_put_value(
                $crate::util::from_arguments_to_cow(format_args!($($args)*)),
                $value,
            )
    };
}
