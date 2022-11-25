use std::str::SplitInclusive;

use crate::styling::{SplitAnsiEscapeCodeInclusive, Styling, CLEAR_STYLE};

pub trait Words {
    fn words(&self) -> WordsIter;
}

impl Words for str {
    fn words(&self) -> WordsIter {
        WordsIter::new(self)
    }
}

#[derive(Clone)]
pub struct WordsIter<'a> {
    words_and_codes: SplitAnsiEscapeCodeInclusive<'a>,
    subwords: SplitInclusive<'a, &'static [char]>,
}

impl<'a> WordsIter<'a> {
    const DELIMITERS: &'static [char] = &[' ', '-', ':', '/', ',', '.'];

    fn new(source: &'a str) -> Self {
        let mut words_and_codes = source.split_ansi_escape_code_inclusive();
        let subwords = words_and_codes
            .next()
            .unwrap_or("")
            .split_inclusive(Self::DELIMITERS);

        Self {
            words_and_codes,
            subwords,
        }
    }
}

impl<'a> Iterator for WordsIter<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(subword) = self.subwords.next() {
            Some(subword)
        } else {
            let code = self.words_and_codes.next()?;
            self.subwords = self
                .words_and_codes
                .next()
                .unwrap()
                .split_inclusive(Self::DELIMITERS);
            Some(code)
        }
    }
}

pub trait Wrap<'a>: Sized + Iterator<Item = &'a str> {
    fn wrap(self, line_width: usize) -> WrapIter<'a, Self>;
}

impl<'a, I: Iterator<Item = &'a str>> Wrap<'a> for I {
    fn wrap(self, line_width: usize) -> WrapIter<'a, Self> {
        WrapIter::new(self, line_width)
    }
}

pub struct WrapIter<'a, I> {
    iter: I,
    line_width: usize,
    line_buf: String,
    overflow_word: Option<&'a str>,
}

impl<I> WrapIter<'_, I> {
    fn new(iter: I, line_width: usize) -> Self {
        assert!(line_width > 0, "line width must not be 0");
        Self {
            iter,
            line_width,
            line_buf: String::new(),
            overflow_word: None,
        }
    }
}

impl<'a, I: Iterator<Item = &'a str>> Iterator for WrapIter<'a, I> {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        let mut char_count = 0;

        loop {
            let word = if let Some(word) = self.overflow_word.take() {
                Some(word)
            } else {
                self.iter.next()
            };

            match word {
                // The word is overflowing the current line, so finish the line and save the
                // overflowing word for future processing when the next line is requested.
                Some(word)
                    if char_count + word.visible_char_indices().count() > self.line_width =>
                {
                    if char_count == 0 {
                        // Line is empty, so the word needs to be broken up.
                        let break_len = self.line_width - char_count;
                        let break_index = word.visible_char_indices().nth(break_len).unwrap().0;
                        let slice = &word[..break_index];
                        self.line_buf += slice;
                        self.overflow_word.replace(&word[break_index..]);
                    } else {
                        // We have processed words for this line previously, so we pass the whole
                        // word for overflow processing for future line requests.
                        self.overflow_word.replace(word);
                    }

                    break;
                }

                // Word fits on the line, append it to the buffer.
                Some(word) => {
                    self.line_buf += word;
                    char_count += word.visible_char_indices().count();
                }

                // No words left, so finish the line if it was started.
                None => {
                    if char_count == 0 {
                        return None;
                    }

                    break;
                }
            }
        }

        let last_code = self
            .line_buf
            .split_ansi_escape_code_inclusive()
            .skip(1)
            .step_by(2)
            .last()
            .map(|v| v.to_string());

        let mut line = self.line_buf.clone();
        if last_code.is_some() {
            line += CLEAR_STYLE
        }

        self.line_buf.clear();
        self.line_buf += &last_code.unwrap_or_default();

        Some(line)
    }
}
