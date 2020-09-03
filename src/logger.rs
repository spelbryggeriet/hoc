use anyhow::Context;
use console::{Style, Term};
use dialoguer::{Confirm, Select};
use std::io::Write;

pub(super) struct Logger {
    stdout: Term,
    stderr: Term,
    new_line_present: bool,
}

impl Logger {
    pub fn new() -> Self {
        Self {
            stdout: Term::stdout(),
            stderr: Term::stderr(),
            new_line_present: true,
        }
    }

    #[cfg(target_os = "not used")]
    pub fn new_line(&mut self) -> anyhow::Result<()> {
        self.stdout.write_line("")?;
        self.new_line_present = true;

        Ok(())
    }

    pub fn info(&mut self, message: impl AsRef<str>) -> anyhow::Result<()> {
        self.stdout.write(b"~ ").context("Writing to stdout")?;
        self.stdout
            .write_line(message.as_ref())
            .context("Writing to stdout")?;
        self.set_new_line_present(message);

        Ok(())
    }

    pub fn status(&mut self, message: impl AsRef<str>) -> anyhow::Result<()> {
        self.stdout.write(b"* ").context("Writing to stdout")?;
        self.stdout
            .write(message.as_ref().as_bytes())
            .context("Writing to stdout")?;
        self.stdout
            .write_line(" ...")
            .context("Writing to stdout")?;
        self.set_new_line_present(message);

        Ok(())
    }

    #[cfg(target_os = "not used")]
    pub fn warning(&mut self, message: impl AsRef<str>) -> anyhow::Result<()> {
        let yellow = Style::new().yellow();
        self.stderr
            .write(yellow.apply_to("! ").to_string().as_bytes())
            .context("Writing to stderr")?;
        self.stderr
            .write_line(&yellow.apply_to(message.as_ref()).to_string())
            .context("Writing to stderr")?;
        self.set_new_line_present(message);

        Ok(())
    }

    pub fn error(&mut self, message: impl AsRef<str>) -> anyhow::Result<()> {
        let red = Style::new().red();
        self.stderr
            .write(red.apply_to("[ERROR] ").to_string().as_bytes())
            .context("Writing to stderr")?;
        self.stderr
            .write_line(&red.apply_to(message.as_ref()).to_string())
            .context("Writing to stderr")?;
        self.set_new_line_present(message);

        Ok(())
    }

    #[cfg(target_os = "not used")]
    pub fn list(
        &mut self,
        message: Option<impl ToString>,
        items: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> anyhow::Result<()> {
        if let Some(message) = message {
            self.info(message.to_string() + ":")?;
        }

        for item in items.into_iter() {
            self.stdout
                .write_line(&("-   ".to_string() + item.as_ref()))
                .context("Writing to stdout")?;
        }

        Ok(())
    }

    pub fn prompt(&mut self, message: impl AsRef<str>) -> anyhow::Result<()> {
        let cyan = Style::new().cyan();
        let want_continue = Confirm::new()
            .with_prompt(
                cyan.apply_to("> ".to_string() + message.as_ref())
                    .to_string(),
            )
            .interact_on(&self.stderr)
            .context("Reading for input")?;
        if !want_continue {
            anyhow::bail!("User cancelled operation");
        }

        Ok(())
    }

    pub fn choose(
        &mut self,
        message: impl AsRef<str>,
        items: impl IntoIterator<Item = impl ToString>,
        default_index: usize,
    ) -> anyhow::Result<usize> {
        let items: Vec<_> = items.into_iter().collect();

        let cyan = Style::new().cyan();
        let index = Select::new()
            .with_prompt(
                cyan.apply_to("# ".to_string() + message.as_ref())
                    .to_string(),
            )
            .items(&items)
            .default(default_index)
            .interact_on_opt(&self.stderr)
            .context("Reading for input")?;

        if let Some(index) = index {
            Ok(index)
        } else {
            anyhow::bail!("User cancelled operation");
        }
    }

    #[cfg(target_os = "not used")]
    pub fn new_line_if_not_present(&mut self) -> anyhow::Result<()> {
        if !self.new_line_present {
            self.new_line()?;
        }

        Ok(())
    }

    fn set_new_line_present(&mut self, message: impl AsRef<str>) {
        self.new_line_present = message
            .as_ref()
            .split('\n')
            .nth_back(1)
            .map(|l| l.trim().is_empty())
            .unwrap_or(false);
    }
}
