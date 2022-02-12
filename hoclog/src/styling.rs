use std::{iter::StepBy, str::CharIndices, vec::IntoIter};

pub const CLEAR_STYLE: &str = "\u{1b}[0m";

pub trait Styling {
    fn split_ansi_escape_code(&self) -> SplitAnsiEscapeCode;
    fn split_ansi_escape_code_inclusive(&self) -> SplitAnsiEscapeCodeInclusive;
    fn visible_char_indices(&self) -> VisibleCharIndices;
    fn active_ansi_escape_code(&self) -> Option<&str>;
}

impl Styling for str {
    fn split_ansi_escape_code(&self) -> SplitAnsiEscapeCode {
        SplitAnsiEscapeCode::new(self)
    }

    fn split_ansi_escape_code_inclusive(&self) -> SplitAnsiEscapeCodeInclusive {
        SplitAnsiEscapeCodeInclusive::new(self)
    }

    fn visible_char_indices(&self) -> VisibleCharIndices {
        VisibleCharIndices::new(self)
    }

    fn active_ansi_escape_code(&self) -> Option<&str> {
        self.split_ansi_escape_code_inclusive()
            .skip(1)
            .step_by(2)
            .last()
    }
}

pub struct VisibleCharIndices<'a> {
    chars: CharIndices<'a>,
    orphan_chars: IntoIter<(usize, char)>,
}

impl<'a> VisibleCharIndices<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            chars: source.char_indices(),
            orphan_chars: Vec::new().into_iter(),
        }
    }
}

impl<'a> Iterator for VisibleCharIndices<'a> {
    type Item = (usize, char);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(c) = self.orphan_chars.next() {
            return Some(c);
        }

        'outer: loop {
            match self.chars.next() {
                Some(esc @ (_, '\x1B')) => match self.chars.next() {
                    Some(bracket @ (_, '[')) => {
                        let chars_clone = self.chars.clone();
                        let mut orphan_chars_count = 0;
                        'inner: loop {
                            match self.chars.next() {
                                Some((_, 'm')) => break 'inner,
                                Some(_) => orphan_chars_count += 1,
                                None => {
                                    self.orphan_chars = [esc, bracket]
                                        .into_iter()
                                        .chain(chars_clone.take(orphan_chars_count))
                                        .collect::<Vec<_>>()
                                        .into_iter();
                                    break 'outer;
                                }
                            }
                        }
                    }
                    Some(c) => {
                        self.orphan_chars = vec![esc, c].into_iter();
                        break 'outer;
                    }
                    None => {
                        self.orphan_chars = vec![esc].into_iter();
                        break 'outer;
                    }
                },
                opt => return opt,
            }
        }

        self.orphan_chars.next()
    }
}

#[derive(Clone)]
pub struct SplitAnsiEscapeCodeInclusive<'a> {
    source: Option<&'a str>,
    escape_code: Option<&'a str>,
}

impl<'a> SplitAnsiEscapeCodeInclusive<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source: Some(source),
            escape_code: None,
        }
    }
}

impl<'a> Iterator for SplitAnsiEscapeCodeInclusive<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        let source = self.source.as_mut()?;

        if let Some(escape_code) = self.escape_code.take() {
            return Some(escape_code);
        }

        let mut char_indices = source.char_indices();

        loop {
            match char_indices.next() {
                Some((start, '\u{1b}')) if matches!(char_indices.next(), Some((_, '['))) => loop {
                    match char_indices.next() {
                        Some((last, c)) if c == 'm' => {
                            let end = last + c.len_utf8();
                            let item = &source[..start];
                            self.escape_code.replace(&source[start..end]);
                            *source = &source[end..];
                            return Some(item);
                        }
                        Some((_, c)) if c.is_ascii_digit() || c == ';' => (),
                        _ => return self.source.take(),
                    }
                },
                None => return self.source.take(),
                _ => (),
            }
        }
    }
}

#[derive(Clone)]
pub struct SplitAnsiEscapeCode<'a> {
    iter: StepBy<SplitAnsiEscapeCodeInclusive<'a>>,
}

impl<'a> SplitAnsiEscapeCode<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            iter: source.split_ansi_escape_code_inclusive().step_by(2),
        }
    }
}

impl<'a> Iterator for SplitAnsiEscapeCode<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}
