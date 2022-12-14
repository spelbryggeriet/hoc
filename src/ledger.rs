use std::{
    borrow::Cow,
    sync::{Mutex, MutexGuard},
};

use once_cell::sync::OnceCell;

use crate::{prelude::*, util::Opt};

pub trait Transaction: Send + 'static {
    fn description(&self) -> Cow<'static, str>;
    fn detail(&self) -> Cow<'static, str>;

    /// Reverts the transaction.
    fn revert(self: Box<Self>) -> anyhow::Result<()>;
}

pub struct Ledger {
    transactions: Vec<Box<dyn Transaction>>,
}

impl Ledger {
    pub fn get_or_init() -> MutexGuard<'static, Self> {
        static LEDGER: OnceCell<Mutex<Ledger>> = OnceCell::new();
        LEDGER
            .get_or_init(|| Mutex::new(Self::new()))
            .lock()
            .expect(EXPECT_THREAD_NOT_POSIONED)
    }

    fn new() -> Self {
        Self {
            transactions: Vec::new(),
        }
    }

    pub fn add(&mut self, transaction: impl Transaction) {
        debug!("Adding transaction: {}", transaction.description());
        self.transactions.push(Box::new(transaction));
    }

    #[throws(anyhow::Error)]
    pub fn rollback(&mut self) {
        if self.transactions.is_empty() {
            return;
        }

        progress!("Rolling back changes");

        let mut always_yes = false;
        while let Some(transaction) = self.transactions.pop() {
            progress!("[Change] {}", transaction.description());
            info!("{}", transaction.detail());

            if !always_yes {
                let yes_to_all = Opt::Custom("Yes to all");
                match select!("Do you want to revert this change?")
                    .with_options([Opt::Yes, yes_to_all, Opt::No])
                    .get()?
                {
                    Opt::Yes => (),
                    Opt::No => break,
                    opt => always_yes = opt == yes_to_all,
                };
            }

            transaction.revert()?;
        }
    }
}
