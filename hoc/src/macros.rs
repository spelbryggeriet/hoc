macro_rules! progress_with_handle {
    ($($args:tt)*) => {{
        $crate::log::progress(format!($($args)*))
    }};
}

macro_rules! progress {
    ($($args:tt)*) => {
        let __handle = progress_with_handle!($($args)*);
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

macro_rules! get {
    ($($args:tt)*) => {
        async {
            let __cow = $crate::util::try_from_arguments_to_key_cow(format_args!($($args)*))?;
            $crate::context::get_context().kv().await.get_item(__cow)
        }
    };
}

macro_rules! put {
    ($value:expr => $($args:tt)*) => {
        async {
            let __cow = $crate::util::try_from_arguments_to_key_cow(format_args!($($args)*))?;
            let __previous = $crate::context::get_context()
                .kv_mut()
                .await
                .put_value(__cow.as_ref(), $value, false)?;
            $crate::ledger::Ledger::get_or_init()
                .lock()
                .await
                .add($crate::context::kv::ledger::Put::new(__cow.into_owned(), __previous).into());
            ::anyhow::Ok(())
        }
    };
}

macro_rules! context_file {
    ($($args:tt)*) => {{
        let __cow = $crate::util::try_from_arguments_to_key_cow(format_args!($($args)*))?;
        $crate::context::FileBuilder::new(__cow)
    }};
}

macro_rules! run {
    ($($args:tt)*) => {{
        let __cow = $crate::util::from_arguments_to_str_cow(format_args!($($args)*));
        $crate::runner::RunBuilder::new(__cow)
    }};
}
