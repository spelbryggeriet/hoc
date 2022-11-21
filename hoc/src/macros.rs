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

macro_rules! run {
    ($fmt:literal $($args:tt)*) => {{
        let cmd = $crate::util::from_arguments_to_str_cow(format_args!($fmt $($args)*));
        $crate::runner::RunBuilder::raw(cmd)
    }};

    ($transactional_cmd:expr) => {{
        $crate::runner::RunBuilder::transactional($transactional_cmd)
    }}
}

macro_rules! _revertible_cmd {
    ($forward_cmd:literal <=> $revert_cmd:literal) => {{
        use ::std::borrow::Cow;

        use ::async_trait::async_trait;

        use $crate::{ledger::Transaction, runner::TransactionalCmd};

        struct RevertibleCmd {
            forward_cmd: Cow<'static, str>,
            revert_cmd: Cow<'static, str>,
        }

        impl TransactionalCmd for RevertibleCmd {
            type Transaction = RevertibleCmdTransaction;

            fn get_transaction(&self) -> Self::Transaction {
                RevertibleCmdTransaction {
                    forward_cmd: self.forward_cmd().into_owned(),
                    revert_cmd: self.revert_cmd().into_owned(),
                }
            }

            fn forward_cmd(&self) -> Cow<str> {
                Cow::Borrowed(&self.forward_cmd)
            }

            fn revert_cmd(&self) -> Cow<str> {
                Cow::Borrowed(&self.revert_cmd)
            }
        }

        struct RevertibleCmdTransaction {
            forward_cmd: String,
            revert_cmd: String,
        }

        #[async_trait]
        impl Transaction for RevertibleCmdTransaction {
            fn description(&self) -> Cow<'static, str> {
                "Run command".into()
            }

            fn detail(&self) -> Cow<'static, str> {
                format!("Command to revert: {}", self.forward_cmd).into()
            }

            async fn revert(self: Box<Self>) -> anyhow::Result<()> {
                run!("{}", self.revert_cmd).await?;
                Ok(())
            }
        }

        let forward_cmd = $crate::util::from_arguments_to_str_cow(format_args!($forward_cmd));
        let revert_cmd = $crate::util::from_arguments_to_str_cow(format_args!($revert_cmd));
        RevertibleCmd {
            forward_cmd,
            revert_cmd,
        }
    }};
}
