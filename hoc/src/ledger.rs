use async_trait::async_trait;
use enum_dispatch::enum_dispatch;
use futures::{stream, Stream};
use once_cell::sync::OnceCell;
use tokio::sync::Mutex;

use crate::{
    context::{files::ledger::Create as FilesCreate, kv::ledger::Put as KvPut},
    prelude::*,
};

#[async_trait]
#[enum_dispatch]
pub trait Transaction {
    fn description(&self) -> &'static str;

    /// Reverts the transaction.
    ///
    /// Running this twice is undefined behavior.
    async fn revert(&mut self) -> anyhow::Result<()>;
}

#[enum_dispatch(Transaction)]
pub enum Actor {
    KvPut,
    FilesCreate,
}

pub struct Ledger {
    actors: Vec<Actor>,
}

impl Ledger {
    pub fn get_or_init() -> &'static Mutex<Self> {
        static LEDGER: OnceCell<Mutex<Ledger>> = OnceCell::new();

        LEDGER.get_or_init(|| Mutex::new(Self::new()))
    }

    fn new() -> Self {
        Self { actors: Vec::new() }
    }

    pub fn add(&mut self, actor: impl Into<Actor>) {
        self.actors.push(actor.into());
    }

    pub fn rollback(&mut self) -> impl Stream<Item = ()> + '_ {
        stream::unfold(self.actors.iter_mut().rev(), |mut iter| async {
            let elem = iter.next()?;
            progress!("{}", elem.description());
            match elem.revert().await {
                Ok(()) => (),
                Err(err) => error!("{err}"),
            }
            Some(((), iter))
        })
    }
}
