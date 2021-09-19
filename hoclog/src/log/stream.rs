use std::sync::Mutex;

use crate::{
    styling::{self, Styling},
    Log,
};

pub struct Stream<'a> {
    log: &'a Log,
    line: Mutex<String>,
}

impl<'a> Stream<'a> {
    pub(super) fn new(log: &'a Log) -> Self {
        Self {
            log,
            line: Mutex::new(String::new()),
        }
    }

    pub fn process(&self, stream: impl AsRef<str>) {
        let mut chunks = stream.as_ref().split('\n');
        let mut line = self.line.lock().unwrap();

        // Always append the first chunk unconditionally.
        *line += chunks.next().unwrap();

        for chunk in chunks {
            let active_code = line.active_ansi_escape_code().map(ToString::to_string);
            if active_code.is_some() {
                *line += styling::CLEAR_STYLE;
            }

            self.log.info(&*line);
            line.clear();

            if let Some(active_code) = active_code {
                *line += &active_code;
            }
            *line += chunk;
        }
    }
}

impl Drop for Stream<'_> {
    fn drop(&mut self) {
        let line = self.line.lock().unwrap();
        if line.len() > 0 {
            self.log.info(&*line);
        }
    }
}
