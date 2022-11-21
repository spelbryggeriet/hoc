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
    ($fmt:literal $(, $id:ident: $type:ty = $init:expr)* $(,)?) => {{
        use ::std::{
            borrow::Cow,
            fmt::{self, Display, Formatter},
        };

        use ::async_trait::async_trait;

        use $crate::{
            prelude::*,
            ledger::Transaction,
            runner::{ManagedCmd, Output},
        };

        struct RevertibleCmd(Cow<'static, str>, RevertibleCmdArguments);

        impl ManagedCmd for RevertibleCmd {
            type Transaction = RevertibleCmdTransaction;

            fn get_transaction(&self, _output: &Output) -> Self::Transaction {
                RevertibleCmdTransaction(self.1.clone())
            }

            fn as_raw(&self) -> Cow<str> {
                Cow::Borrowed(&self.0)
            }
        }

        impl Display for RevertibleCmd {
            #[throws(fmt::Error)]
            fn fmt(&self, f: &mut Formatter) {
                self.0.fmt(f)?;
            }
        }

        struct RevertibleCmdTransaction(RevertibleCmdArguments);

        #[async_trait]
        impl Transaction for RevertibleCmdTransaction {
            fn description(&self) -> Cow<'static, str> {
                "Run command".into()
            }

            fn detail(&self) -> Cow<'static, str> {
                format!("Command to revert: {}", self.0).into()
            }

            async fn revert(self: Box<Self>) -> anyhow::Result<()> {
                run!("{}", self.0).await?;
                Ok(())
            }
        }

        #[derive(Clone)]
        struct RevertibleCmdArguments {$(
            $id: $type,
        )*}

        impl Display for RevertibleCmdArguments {
            #[throws(fmt::Error)]
            fn fmt(&self, f: &mut Formatter) {
                $(
                let $id = &self.$id;
                )*

                write!(f, $fmt)?;
            }
        }

        |raw| RevertibleCmd(raw, RevertibleCmdArguments {
            $($id: $init),*
        })
    }};
}
