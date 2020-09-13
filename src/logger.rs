use anyhow::Context;
use console::{Style, Term};
use dialoguer::{Confirm, Input, Password, Select};
use std::io::Write;

use crate::prelude::*;

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

    pub fn _new_line(&mut self) -> AppResult<()> {
        self.stdout.write_line("")?;
        self.new_line_present = true;

        Ok(())
    }

    pub fn info(&mut self, message: impl AsRef<str>) -> AppResult<()> {
        self.stdout.write(b"~ ").context("Writing to stdout")?;
        self.stdout
            .write_line(message.as_ref())
            .context("Writing to stdout")?;
        self.set_new_line_present(message);

        Ok(())
    }

    pub fn status(&mut self, message: impl AsRef<str>) -> AppResult<()> {
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

    pub fn _warning(&mut self, message: impl AsRef<str>) -> AppResult<()> {
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

    pub fn error(&mut self, message: impl AsRef<str>) -> AppResult<()> {
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

    pub fn _list(
        &mut self,
        message: Option<impl ToString>,
        items: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> AppResult<()> {
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

    pub fn prompt(&mut self, message: impl AsRef<str>) -> AppResult<()> {
        let cyan = Style::new().cyan();
        let want_continue = Confirm::new()
            .with_prompt(
                cyan.apply_to("🤨 ".to_string() + message.as_ref())
                    .to_string(),
            )
            .interact_on(&self.stderr)
            .context("Reading for input")?;
        if !want_continue {
            anyhow::bail!("User cancelled operation");
        }

        Ok(())
    }

    pub fn input(&mut self, message: impl AsRef<str>) -> AppResult<String> {
        Input::new()
            .with_prompt(
                Style::new()
                    .cyan()
                    .apply_to("🤓 ".to_string() + message.as_ref())
                    .to_string(),
            )
            .interact_on(&self.stderr)
            .context("Reading for input")
    }

    pub fn hidden_input(&mut self, message: impl AsRef<str>) -> AppResult<String> {
        Password::new()
            .with_prompt(
                Style::new()
                    .cyan()
                    .apply_to("🤓 ".to_string() + message.as_ref())
                    .to_string(),
            )
            .interact_on(&self.stderr)
            .context("Reading for hidden input")
    }

    pub fn choose(
        &mut self,
        message: impl AsRef<str>,
        items: impl IntoIterator<Item = impl ToString>,
        default_index: usize,
    ) -> AppResult<usize> {
        let items: Vec<_> = items.into_iter().collect();

        let cyan = Style::new().cyan();
        let index = Select::new()
            .with_prompt(
                cyan.apply_to("🤔 ".to_string() + message.as_ref())
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

    pub fn _new_line_if_not_present(&mut self) -> AppResult<()> {
        if !self.new_line_present {
            self._new_line()?;
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
