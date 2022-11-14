use std::{
    io::{self, Write},
    ops::{Deref, DerefMut},
};

use crossterm::{cursor, queue, terminal, QueueableCommand};

use crate::{log::Error, prelude::*};

struct Stdout(io::Stdout);

impl Stdout {
    fn get() -> Self {
        Self(io::stdout())
    }
}

impl Deref for Stdout {
    type Target = io::Stdout;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Stdout {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for Stdout {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

#[throws(Error)]
pub fn move_cursor_down(lines_down: u16) {
    let mut stdout = Stdout::get();

    if lines_down > 0 {
        stdout.queue(cursor::MoveToNextLine(lines_down))?;
    }
}

#[throws(Error)]
pub fn move_cursor_up(lines_up: u16) {
    let mut stdout = Stdout::get();

    if lines_up > 0 {
        stdout.queue(cursor::MoveToPreviousLine(lines_up))?;
    }
}

#[throws(Error)]
pub fn move_cursor_to_column(column: u16) {
    let mut stdout = Stdout::get();

    stdout.queue(cursor::MoveToColumn(column))?;
}

#[throws(Error)]
pub fn clear(lines_up: u16) {
    let mut stdout = Stdout::get();

    stdout.queue(cursor::MoveToColumn(0))?;

    if lines_up > 0 {
        for _ in 0..lines_up {
            queue!(
                stdout,
                terminal::Clear(terminal::ClearType::UntilNewLine),
                cursor::MoveToPreviousLine(1),
            )?;
        }
    }
}
