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
    ($($args:tt)*) => {{
        let cmd = $crate::util::from_arguments_to_str_cow(format_args!($($args)*));
        $crate::runner::RunBuilder::new(cmd)
    }};
}

macro_rules! revert_cmd {
    (|$output:ident, $($id:ident: $type:ty = $init:expr,)*| $body:expr $(,)?) => {{
        use ::std::borrow::Cow;

        use ::async_trait::async_trait;

        use $crate::{
            ledger::Transaction,
            runner::{ManagedCmd, Output},
        };

        struct RevertibleCmd<F>(
            Cow<'static, str>,
            RevertibleCmdArguments,
            F,
        );

        impl<F> ManagedCmd for RevertibleCmd<F>
        where
            F: for<'a> Fn(&'a Output $(, &'a $type)*) -> Cow<'a, str> + Send + Sync + 'static
        {
            type Transaction = RevertibleCmdTransaction;

            fn get_transaction(&self, output: &Output) -> Self::Transaction {
                RevertibleCmdTransaction {
                    original_cmd: self.as_raw().into_owned(),
                    revert_cmd: self.revert_cmd(output).into_owned(),
                }
            }

            fn as_raw(&self) -> Cow<str> {
                Cow::Borrowed(&self.0)
            }

            fn revert_cmd<'a>(&'a self, $output: &'a Output) -> Cow<'a, str> {
                $(
                let $id = &self.1.$id;
                )*

                self.2($output $(, $id)*)
            }
        }

        struct RevertibleCmdTransaction {
            original_cmd: String,
            revert_cmd: String,
        }

        #[async_trait]
        impl Transaction for RevertibleCmdTransaction {
            fn description(&self) -> Cow<'static, str> {
                "Run command".into()
            }

            fn detail(&self) -> Cow<'static, str> {
                format!("Command to revert: {}", self.original_cmd).into()
            }

            async fn revert(self: Box<Self>) -> anyhow::Result<()> {
                run!("{}", self.revert_cmd).await?;
                Ok(())
            }
        }

        struct RevertibleCmdArguments {$(
            $id: $type,
        )*}

        fn generate_cmd<'a>($output: &'a Output $(, $id: &'a $type)*) -> Cow<'a, str> {
            ($body).into()
        }

        |raw| RevertibleCmd(
            raw,
            RevertibleCmdArguments {
                $($id: $init,)*
            },
            generate_cmd,
        )
    }};

    ($($fmt:tt)*) => {{
        revert_cmd!(|_output, cmd: String = format!($($fmt)*),| cmd)
    }};
}
