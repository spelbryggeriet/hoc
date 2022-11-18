use std::borrow::Cow;

use crate::{
    context::{self, key::Key, kv},
    ledger::Ledger,
    prelude::*,
};

#[throws(anyhow::Error)]
pub async fn put<V: Into<kv::Value>>(key: Cow<'static, Key>, value: V) {
    let previous_value =
        context::get_context()
            .kv_mut()
            .await
            .put_value(key.as_ref(), value, false)?;

    if previous_value.is_none() || previous_value != Some(None) {
        Ledger::get_or_init().lock().await.add(kv::ledger::Put::new(
            key.into_owned(),
            previous_value.flatten(),
        ));
    }
}

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
        $crate::macros::put(
            $crate::util::try_from_arguments_to_key_cow(format_args!($($args)*))?,
            $value,
        )
    };
}

macro_rules! context_file {
    ($($args:tt)*) => {{
        let __cow = $crate::util::try_from_arguments_to_key_cow(format_args!($($args)*))?;
        $crate::context::fs::FileBuilder::new(__cow)
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

macro_rules! run {
    ($($args:tt)*) => {{
        let __cow = $crate::util::from_arguments_to_str_cow(format_args!($($args)*));
        $crate::runner::RunBuilder::new(__cow)
    }};
}
