use crate::log::styling::{SplitAnsiEscapeCodeInclusive, CLEAR_STYLE};
use crate::prelude::*;
use std::mem;

pub trait Wrapping {
    fn words_inclusive(&self) -> WordsInclusive;
    fn wrapped_lines(&self, line_width: usize) -> WrappedLines;
}

impl Wrapping for str {
    fn words_inclusive(&self) -> WordsInclusive {
        WordsInclusive::new(self)
    }

    fn wrapped_lines(&self, line_width: usize) -> WrappedLines {
        WrappedLines::new(self, line_width)
    }
}

#[derive(Clone)]
pub struct WordsInclusive<'a> {
    source: &'a str,
    end: usize,
    words_and_codes: SplitAnsiEscapeCodeInclusive<'a>,
}

impl<'a> WordsInclusive<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            end: 0,
            words_and_codes: source.split_ansi_escape_code_inclusive(),
        }
    }
}

impl<'a> Iterator for WordsInclusive<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.end == 0 {
            self.end = self.words_and_codes.next()?.len();
        };

        let maybe_sub_word = self.source[..self.end]
            .char_indices()
            .find(|(_, c)| [' ', '-', ':', '/', ',', '.'].contains(&c));

        let end;
        if let Some((last, c)) = maybe_sub_word {
            end = last + c.len_utf8();
            self.end -= end;
        } else {
            end = self.end + self.words_and_codes.next().unwrap_or_default().len();
            self.end = 0;
        }

        let item = &self.source[..end];
        self.source = &self.source[end..];

        Some(item)
    }
}

#[derive(Clone)]
pub struct WrappedLines<'a> {
    words: WordsInclusive<'a>,
    line_width: usize,
    line_buf: String,
    overflow_word: Option<&'a str>,
    last_code: Option<&'a str>,
}

impl<'a> WrappedLines<'a> {
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

impl<'a> Iterator for WrappedLines<'a> {
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
