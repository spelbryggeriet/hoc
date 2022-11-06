use std::{
    fmt::{self, Display, Formatter},
    io::{self, Write},
    iter,
    ops::{Add, Sub},
};

use crossterm::{
    queue,
    style::{self, Color},
    terminal, QueueableCommand,
};

use crate::{log::Error, prelude::*};

fn render(
    color: Option<Color>,
    content: &str,
    position: &mut Position,
    lines: &mut Vec<(Line, Vec<ColorSpan>)>,
) {
    if content.is_empty() {
        return;
    }

    let (line, color_spans) = &mut lines[position.row];
    let current_char_count = line.content.chars().count();
    let start_column = position.column;
    let end_column = start_column + content.chars().count();
    if end_column > current_char_count {
        line.content
            .extend(iter::repeat(' ').take(end_column - current_char_count))
    }

    let chars = line.content.chars().map(char::len_utf8);
    let start_index: usize = chars.clone().take(start_column).sum();
    let end_index: usize = chars.take(end_column).sum();

    line.content.replace_range(start_index..end_index, content);
    line.empty = false;

    if let Some(new_color) = color {
        let new_color_span = ColorSpan {
            start: start_column,
            end: end_column,
            color: new_color,
        };

        let mut color_span_to_add = None;
        color_spans.retain_mut(|color_span| {
            // Perform a bounds check on the new line against this current color span.
            let start_within_bounds =
                start_column >= color_span.start && start_column < color_span.end;
            let end_within_bounds = end_column > color_span.start && end_column <= color_span.end;

            if start_within_bounds {
                // The new color span starts at or after the current color span.

                if end_within_bounds {
                    // Here it also ends before or at the current color span, so we don't have
                    // to continue further.

                    if color_span.end != end_column {
                        // An additional color span needs to be created, since the new one is
                        // strictly within the bounds of the current one. This one starts at the
                        // end of the new one.
                        color_span_to_add.replace(ColorSpan {
                            start: end_column,
                            ..*color_span
                        });
                    }
                }

                if color_span.start != start_column {
                    // The current color span is truncated to end where the new one begins.
                    color_span.end = start_column;
                    true
                } else {
                    // The current color span is truncated to zero width, so we remove it
                    // instead.
                    false
                }
            } else if end_within_bounds {
                // The new color span ends before or at the current color span, so we can stop
                // here.

                if color_span.end != end_column {
                    // The current color span is truncated to start where the new one ends.
                    color_span.start = end_column;
                    true
                } else {
                    // The current color span is truncated to zero width, so we remove it
                    // instead.
                    false
                }
            } else if color_span.start > start_column && color_span.end < end_column {
                // The current color span is strictly within the bounds of the new one, so it
                // is effectively overwritten.
                false
            } else {
                // The two spans do not overlap so we do nothing.
                true
            }
        });

        color_spans.push(new_color_span);
        color_spans.extend(color_span_to_add);
        color_spans.sort_by_key(|color_span| color_span.start);

        for i in (1..color_spans.len()).rev() {
            let left = color_spans[i - 1];
            let right = color_spans[i];

            if left.end == right.start && left.color == right.color {
                color_spans[i - 1].end = right.end;
                color_spans.remove(i);
            }
        }
    }

    position.move_to_column(end_column);
}

fn assert_subview(
    origin: Position,
    parent_max_width: usize,
    parent_max_height: Option<usize>,
    subview_max_width: usize,
    subview_max_height: Option<usize>,
) {
    assert!(
        origin.column < parent_max_width,
        "origin {origin} must be within the width {parent_max_width} of the parent view",
    );
    assert!(
        subview_max_width <= parent_max_width,
        "max width {subview_max_width} of subview must be less or equal to the max width {parent_max_width} of the parent view",
    );

    if let Some(parent_max_height) = parent_max_height {
        assert!(
            origin.row < parent_max_height,
            "origin {origin} must be within the height {parent_max_height} of the parent view",
        );

        let Some(subview_max_height) = subview_max_height else {
            panic!("subview height must not be infinite if parent height is finite");
        };

        assert!(
            subview_max_height < parent_max_height,
            "max height {subview_max_height} of subview must be less or equal to the max height {parent_max_height} of the parent view"
        );
    }
}

pub trait View {
    fn render(&mut self, color: Option<Color>, content: &str);
    fn subview(&mut self, offset: Position, max_width: usize, max_height: Option<usize>)
        -> Subview;
    fn max_height(&self) -> Option<usize>;
    fn max_width(&self) -> usize;
    fn position(&self) -> Position;
    fn position_mut(&mut self) -> &mut Position;
}

#[derive(Debug)]
pub struct RootView {
    pub position: Position,
    lines: Vec<(Line, Vec<ColorSpan>)>,
    height: usize,
    max_height: Option<usize>,
    max_width: usize,
}

impl RootView {
    pub fn new(max_width: usize) -> Self {
        Self {
            lines: Vec::new(),
            height: 0,
            max_height: None,
            max_width,
            position: Position::new(0, 0),
        }
    }

    pub fn set_max_height(&mut self, width: usize) {
        self.max_height.replace(width);
    }

    pub fn set_infinite_height(&mut self) {
        self.max_height.take();
    }

    pub fn set_max_width(&mut self, width: usize) {
        self.max_width = width;
    }

    fn extend_line_buffer(&mut self) {
        if self.position.row >= self.lines.len() {
            self.lines.extend(
                iter::repeat(Default::default()).take(self.position.row - self.lines.len() + 1),
            )
        }
        self.height = (self.position.row + 1).max(self.height);
    }

    #[throws(Error)]
    pub fn print(&mut self) -> usize {
        self.extend_line_buffer();

        if self.height == 0 {
            return 0;
        }

        let mut stdout = io::stdout();

        for (i, (line, color_spans)) in self.lines.iter().take(self.height).enumerate() {
            if i > 0 {
                stdout.queue(style::Print("\n"))?;
            }

            if line.empty {
                continue;
            }

            let mut start = 0;
            for color_span in color_spans {
                let chars = line.content.chars().map(char::len_utf8);
                let color_start_index: usize = chars.clone().take(color_span.start).sum();
                let color_end_index: usize = chars.take(color_span.end).sum();

                queue!(
                    stdout,
                    style::Print(&line.content[start..color_start_index]),
                    style::SetForegroundColor(color_span.color),
                    style::Print(&line.content[color_start_index..color_end_index]),
                    style::SetForegroundColor(Color::Reset),
                )?;

                start = color_end_index;
            }

            queue!(
                stdout,
                style::Print(&line.content[start..]),
                terminal::Clear(terminal::ClearType::UntilNewLine),
            )?;
        }

        stdout.flush()?;

        let print_height = self.height;

        self.lines.iter_mut().for_each(|(l, c)| {
            l.content.clear();
            l.empty = true;
            c.clear()
        });
        self.height = 0;
        self.position = Position::new(0, 0);

        return print_height;
    }
}

impl View for RootView {
    fn render(&mut self, color: Option<Color>, content: &str) {
        self.extend_line_buffer();
        render(color, content, &mut self.position, &mut self.lines);
    }

    fn subview(
        &mut self,
        offset: Position,
        max_width: usize,
        max_height: Option<usize>,
    ) -> Subview {
        assert_subview(
            offset,
            self.max_width,
            self.max_height,
            max_width,
            max_height,
        );
        Subview::new(offset, max_width, max_height, &mut self.lines)
    }

    fn max_height(&self) -> Option<usize> {
        self.max_height
    }

    fn max_width(&self) -> usize {
        self.max_width
    }

    fn position(&self) -> Position {
        self.position
    }

    fn position_mut(&mut self) -> &mut Position {
        &mut self.position
    }
}

pub struct Subview<'a> {
    pub position: Position,
    origin: Position,
    lines: &'a mut Vec<(Line, Vec<ColorSpan>)>,
    max_height: Option<usize>,
    max_width: usize,
}

impl<'a> Subview<'a> {
    fn new(
        origin: Position,
        max_width: usize,
        max_height: Option<usize>,
        lines: &'a mut Vec<(Line, Vec<ColorSpan>)>,
    ) -> Self {
        Self {
            position: Position::new(0, 0),
            origin,
            lines,
            max_height,
            max_width,
        }
    }
}

impl View for Subview<'_> {
    fn render(&mut self, color: Option<Color>, content: &str) {
        let mut real_position = self.origin + self.position;
        render(color, content, &mut real_position, self.lines);
        self.position = real_position - self.origin;
    }

    fn subview(
        &mut self,
        offset: Position,
        max_width: usize,
        max_height: Option<usize>,
    ) -> Subview {
        assert_subview(
            offset,
            self.max_width,
            self.max_height,
            max_width,
            max_height,
        );
        Subview::new(self.origin + offset, max_width, max_height, self.lines)
    }

    fn max_height(&self) -> Option<usize> {
        self.max_height
    }

    fn max_width(&self) -> usize {
        self.max_width
    }

    fn position(&self) -> Position {
        self.position
    }

    fn position_mut(&mut self) -> &mut Position {
        &mut self.position
    }
}

#[derive(Debug, Clone)]
struct Line {
    content: String,
    empty: bool,
}

impl Default for Line {
    fn default() -> Self {
        Self {
            content: String::default(),
            empty: true,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ColorSpan {
    start: usize,
    end: usize,
    color: Color,
}

#[derive(Debug, Copy, Clone)]
pub struct Position {
    row: usize,
    column: usize,
}

impl Position {
    pub fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }

    pub fn row(&self) -> usize {
        self.row
    }

    pub fn move_down(&mut self, lines: usize) {
        self.row += lines;
    }

    pub fn move_to_column(&mut self, column: usize) {
        self.column = column;
    }
}

impl Display for Position {
    #[throws(fmt::Error)]
    fn fmt(&self, f: &mut Formatter) {
        write!(f, "({},{})", self.row, self.column)?;
    }
}

impl Add for Position {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            row: self.row + rhs.row,
            column: self.column + rhs.column,
        }
    }
}

impl Sub for Position {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self {
            row: self.row - rhs.row,
            column: self.column - rhs.column,
        }
    }
}
