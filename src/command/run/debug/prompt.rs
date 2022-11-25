use crate::prelude::*;

#[throws(anyhow::Error)]
pub fn run() {
    let mut runner = PromptRunner::new();
    runner.run_suite()?;

    let mut progresses = Vec::with_capacity(10);
    for progress in 1..=10 {
        progresses.push(progress_with_handle!("Progress {progress}"));
        runner.run_suite()?;
    }
}

struct PromptRunner(usize);

impl PromptRunner {
    fn new() -> Self {
        Self(0)
    }

    #[throws(anyhow::Error)]
    fn run<T, E: Into<anyhow::Error>>(&mut self, f: impl FnOnce(usize) -> Result<T, E>) -> T {
        self.0 += 1;

        warn!("[Prompt {}]::Before", self.0);
        let r = f(self.0);
        warn!("[Prompt {}]::After", self.0);
        r.map_err(E::into)?
    }

    #[throws(anyhow::Error)]
    fn run_suite(&mut self) {
        let _: String = self.run(|i| prompt!("Prompt {i}").get())?;
        let _: String = self.run(|i| prompt!("Prompt {i}").with_default("default").get())?;
        let _: String = self.run(|i| prompt!("Prompt {i}").with_initial_input("initial").get())?;
        let _: Secret<String> = self.run(|i| prompt!("Prompt {i}").secret().get())?;
        self.run(|i| select!("Prompt {i}?").with_option("Option 1").get())?;
        self.run(|i| {
            select!("Prompt {i}?")
                .with_option("Option 1")
                .with_option("Option 2")
                .get()
        })?;
    }
}
