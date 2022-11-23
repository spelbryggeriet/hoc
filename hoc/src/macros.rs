use crate::{
    context::{self, key::KeyOwned, kv},
    ledger::Ledger,
    prelude::*,
};

#[throws(anyhow::Error)]
pub async fn put<K: Into<KeyOwned>, V: Into<kv::Value>>(key: K, value: V) {
    let key = key.into();
    let value = value.into();

    let previous_value =
        context::get_context()
            .kv_mut()
            .await
            .put_value(&key, value.clone(), false)?;

    if previous_value != Some(None) {
        Ledger::get_or_init().lock().await.add(kv::ledger::Put::new(
            key,
            value,
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

macro_rules! get {
    (move $item:expr => $($args:tt)*) => {{
        let key = $crate::util::from_arguments_to_str_cow(format_args!($($args)*));
        let item = $item;
        item.take(&key)
    }};

    ($item:expr => $($args:tt)*) => {{
        let key = $crate::util::from_arguments_to_str_cow(format_args!($($args)*));
        let item = $item;
        item.get(&key)
    }};

    ($($args:tt)*) => {
        async {
            let key = $crate::util::from_arguments_to_str_cow(format_args!($($args)*));
            $crate::context::get_context().kv().await.get_item(&key)
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
