use std::borrow::Cow;

use async_trait::async_trait;
use futures::{stream, Stream};
use once_cell::sync::OnceCell;
use tokio::sync::Mutex;

use crate::prelude::*;

#[async_trait]
pub trait Transaction: Send + 'static {
    fn description(&self) -> Cow<'static, str>;
    fn detail(&self) -> Cow<'static, str>;

    /// Reverts the transaction.
    ///
    /// Running this twice is undefined behavior.
    async fn revert(self: Box<Self>) -> anyhow::Result<()>;
}

pub struct Ledger {
    actors: Vec<Box<dyn Transaction>>,
}

impl Ledger {
    pub fn get_or_init() -> &'static Mutex<Self> {
        static LEDGER: OnceCell<Mutex<Ledger>> = OnceCell::new();
        LEDGER.get_or_init(|| Mutex::new(Self::new()))
    }

    fn new() -> Self {
        Self { actors: Vec::new() }
    }

    pub fn add(&mut self, actor: impl Transaction) {
        self.actors.push(Box::new(actor));
    }

    pub fn rollback(&mut self) -> impl Stream<Item = ()> + '_ {
        stream::unfold(self.actors.drain(..).rev(), |mut iter| async {
            let elem = iter.next()?;

            progress!("{}", elem.description());
            info!("{}", elem.detail());

            match elem.revert().await {
                Ok(()) => (),
                Err(err) => error!("{err}"),
            }
            Some(((), iter))
        })
    }
}
