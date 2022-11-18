use crate::{
    context::{self, key::KeyOwned, kv},
    ledger::Ledger,
    prelude::*,
};

#[throws(anyhow::Error)]
pub async fn put<K: Into<KeyOwned>, V: Into<kv::Value>>(key: K, value: V) {
    let key = key.into();

    let previous_value = context::get_context()
        .kv_mut()
        .await
        .put_value(&key, value, false)?;

    if previous_value.is_none() || previous_value != Some(None) {
        Ledger::get_or_init()
            .lock()
            .await
            .add(kv::ledger::Put::new(key, previous_value.flatten()));
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
        let __msg = $crate::util::from_arguments_to_str_cow(format_args!($($args)*));
        $crate::prompt::PromptBuilder::new(__msg)
    }};
}

macro_rules! select {
    ($($args:tt)*) => {{
        let __msg = $crate::util::from_arguments_to_str_cow(format_args!($($args)*));
        $crate::prompt::SelectBuilder::new(__msg)
    }};
}

macro_rules! get {
    ($($args:tt)*) => {
        async {
            let __key = $crate::util::from_arguments_to_str_cow(format_args!($($args)*));
            $crate::context::get_context().kv().await.get_item(&__key)
        }
    };
}

macro_rules! put {
    ($value:expr => $($args:tt)*) => {
        $crate::macros::put(
            &$crate::util::from_arguments_to_str_cow(format_args!($($args)*)),
            $value,
        )
    };
}

macro_rules! context_file {
    ($($args:tt)*) => {{
        let __key = $crate::util::from_arguments_to_key_cow(format_args!($($args)*));
        $crate::context::fs::FileBuilder::new(__key)
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
        let __cmd = $crate::util::from_arguments_to_str_cow(format_args!($($args)*));
        $crate::runner::RunBuilder::new(__cmd)
    }};
}
