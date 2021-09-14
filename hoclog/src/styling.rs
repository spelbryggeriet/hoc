pub const CLEAR_STYLE: &str = "\u{1b}[0m";

pub trait Styling {
    fn split_ansi_escape_code(&self) -> SplitAnsiEscapeCode;
    fn split_ansi_escape_code_inclusive(&self) -> SplitAnsiEscapeCodeInclusive;
    fn char_count_without_styling(&self) -> usize;
    fn is_ansi_escape_code(&self) -> bool;
    fn active_ansi_escape_code(&self) -> Option<&str>;
    fn normalize_styling(&self) -> String;
}

impl Styling for str {
    fn split_ansi_escape_code(&self) -> SplitAnsiEscapeCode {
        SplitAnsiEscapeCode::new(self)
    }

    fn split_ansi_escape_code_inclusive(&self) -> SplitAnsiEscapeCodeInclusive {
        SplitAnsiEscapeCodeInclusive::new(self)
    }

    fn char_count_without_styling(&self) -> usize {
        self.split_ansi_escape_code()
            .fold(0, |count, word| count + word.chars().count())
    }

    fn is_ansi_escape_code(&self) -> bool {
        self.split_ansi_escape_code_inclusive().count() == 3
    }

    fn active_ansi_escape_code(&self) -> Option<&str> {
        self.split_ansi_escape_code_inclusive()
            .skip(1)
            .step_by(2)
            .last()
    }

    fn normalize_styling(&self) -> String {
        let mut iter = self.split_ansi_escape_code_inclusive();
        let mut normalized = iter.next().unwrap().to_string();
        let codes = iter.clone().step_by(2);
        let words = iter.skip(1).step_by(2);
        let iter = codes.zip(words).scan(None, |active_style, (code, word)| {
            if *active_style == Some(code) || word.is_empty() {
                Some(["", word])
            } else {
                active_style.replace(code);
                Some([code, word])
            }
        });

        for [code, word] in iter {
            normalized += code;
            normalized += word;
        }

        normalized
    }
}

#[derive(Clone)]
pub struct SplitAnsiEscapeCode<'a> {
    source: Option<&'a str>,
}

impl<'a> SplitAnsiEscapeCode<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source: Some(source),
        }
    }
}

impl<'a> Iterator for SplitAnsiEscapeCode<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        let source = self.source.as_mut()?;

        let mut char_indices = source.char_indices();

        loop {
            match char_indices.next() {
                Some((start, '\u{1b}')) if matches!(char_indices.next(), Some((_, '['))) => loop {
                    match char_indices.next() {
                        Some((last, c)) if c == 'm' => {
                            let end = last + c.len_utf8();
                            let item = &source[..start];
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
