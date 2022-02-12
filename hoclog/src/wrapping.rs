use std::{mem, str::SplitInclusive};

use crate::styling::{SplitAnsiEscapeCodeInclusive, Styling, CLEAR_STYLE};

pub trait Wrapping {
    fn words_inclusive(&self) -> WordsInclusive;
    fn wrapped_words(&self, line_width: usize) -> WrappedWords;
}

impl Wrapping for str {
    fn words_inclusive(&self) -> WordsInclusive {
        WordsInclusive::new(self)
    }

    fn wrapped_words(&self, line_width: usize) -> WrappedWords {
        WrappedWords::new(self, line_width)
    }
}

#[derive(Clone)]
pub struct WordsInclusive<'a> {
    words_and_codes: SplitAnsiEscapeCodeInclusive<'a>,
    subwords: SplitInclusive<'a, &'static [char]>,
}

impl<'a> WordsInclusive<'a> {
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

impl<'a> Iterator for WordsInclusive<'a> {
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

#[derive(Clone)]
pub struct WrappedWords<'a> {
    words: WordsInclusive<'a>,
    line_width: usize,
    line_buf: String,
    overflow_word: Option<&'a str>,
    last_code: Option<&'a str>,
}

impl<'a> WrappedWords<'a> {
    fn new(source: &'a str, line_width: usize) -> Self {
        assert!(line_width > 0, "line width must not be 0");
        Self {
            words: source.words_inclusive(),
            line_width,
            line_buf: String::with_capacity(mem::size_of::<char>() * line_width),
            overflow_word: None,
            last_code: None,
        }
    }
}

impl<'a> Iterator for WrappedWords<'a> {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        let mut char_count = 0;

        loop {
            let maybe_word = if let Some(word) = self.overflow_word.take() {
                Some(word)
            } else {
                self.words.next()
            };

            match maybe_word {
                // Word is an ANSI escape code, so it is effectively ignored, since it doesn't print
                // visibly.
                Some(word) if word.is_ansi_escape_code() => {
                    self.line_buf += word;

                    if word == CLEAR_STYLE {
                        self.last_code.take();
                    } else {
                        self.last_code.replace(word);
                    }
                }

                // The word is overflowing the current line, so finish the line and save the
                // overflowing word for future processing when the next line is requested.
                Some(word) if char_count + word.chars().count() > self.line_width => {
                    if char_count == 0 {
                        // Line is empty, so the word needs to be broken up.
                        let break_len = self.line_width - char_count;
                        let break_index = word.char_indices().nth(break_len).unwrap().0;
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
                    char_count += word.chars().count();
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

        if self.last_code.is_some() {
            self.line_buf += CLEAR_STYLE
        }

        let line = self.line_buf.clone();
        self.line_buf.clear();
        self.line_buf += self.last_code.unwrap_or_default();

        Some(line)
    }
}
