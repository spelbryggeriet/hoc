use async_trait::async_trait;
use enum_dispatch::enum_dispatch;
use futures::{stream, Stream};
use once_cell::sync::OnceCell;
use tokio::sync::Mutex;

use crate::context::kv::ledger::Put as KvPut;

#[async_trait]
#[enum_dispatch]
pub trait Transaction {
    /// Reverts the transaction.
    ///
    /// Running this twice is undefined behavior.
    async fn revert(&mut self) -> Result<(), anyhow::Error>;
}

#[enum_dispatch(Transaction)]
pub enum Actor {
    KvPut,
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

    pub fn add(&mut self, actor: Actor) {
        self.actors.push(actor);
    }

    pub fn rollback(&mut self) -> impl Stream<Item = anyhow::Result<()>> + '_ {
        stream::unfold(self.actors.iter_mut(), |mut iter| async {
            Some((iter.next()?.revert().await, iter))
        })
    }
}
