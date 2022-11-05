use std::{
    io::{self, Write},
    iter,
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
    height: &mut usize,
) {
    if content.is_empty() {
        return;
    }

    extend_line_buffer(position, lines, height);

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
                    false
                } else {
                    // The current color span is truncated to zero width, so we remove it
                    // instead.
                    true
                }
            } else if end_within_bounds {
                // The new color span ends before or at the current color span, so we can stop
                // here.

                if color_span.end != end_column {
                    // The current color span is truncated to start where the new one ends.
                    color_span.start = end_column;
                    false
                } else {
                    // The current color span is truncated to zero width, so we remove it
                    // instead.
                    true
                }
            } else {
                // The current color span is strictly within the bounds of the new one, so it
                // is effectively overwritten.
                true
            }
        });

        color_spans.push(new_color_span);
        color_spans.extend(color_span_to_add);
        color_spans.sort_by_key(|color_span| color_span.start);
    }

    position.move_to_column(end_column);
}

fn extend_line_buffer(
    position: &mut Position,
    lines: &mut Vec<(Line, Vec<ColorSpan>)>,
    height: &mut usize,
) {
    if position.row >= lines.len() {
        lines.extend(iter::repeat(Default::default()).take(position.row - lines.len() + 1))
    }
    *height = (position.row + 1).max(*height);
}

pub trait View {
    fn render(&mut self, color: Option<Color>, content: &str);
    fn set_max_height(&mut self, height: usize);
    fn max_height(&self) -> Option<usize>;
    fn max_width(&self) -> usize;
    fn position(&self) -> Position;
    fn position_mut(&mut self) -> &mut Position;
}

#[derive(Debug)]
pub struct RootView {
    lines: Vec<(Line, Vec<ColorSpan>)>,
    height: usize,
    max_height: Option<usize>,
    max_width: usize,
    pub position: Position,
}

impl RootView {
    pub fn new(max_width: usize) -> Self {
        Self {
            lines: Vec::new(),
            height: 0,
            max_height: None,
            max_width,
            position: Position::new(),
        }
    }

    pub fn set_infinite_height(&mut self) {
        self.max_height.take();
    }

    pub fn set_max_width(&mut self, width: usize) {
        self.max_width = width;
    }

    #[throws(Error)]
    pub fn print(&mut self) -> usize {
        extend_line_buffer(&mut self.position, &mut self.lines, &mut self.height);

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
        self.position = Position::new();

        return print_height;
    }
}

impl View for RootView {
    fn render(&mut self, color: Option<Color>, content: &str) {
        render(
            color,
            content,
            &mut self.position,
            &mut self.lines,
            &mut self.height,
        );
    }

    fn set_max_height(&mut self, height: usize) {
        self.max_height.replace(height);
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
    fn new() -> Self {
        Self { row: 0, column: 0 }
    }

    pub fn row(&self) -> usize {
        self.row
    }

    pub fn move_down(&mut self, lines: usize) {
        self.row += lines;
    }

    pub fn move_up(&mut self, lines: usize) {
        debug_assert!(self.row >= lines, "moving up beyond the zeroth line");
        self.row = self.row.saturating_sub(lines);
    }

    pub fn move_to_column(&mut self, column: usize) {
        self.column = column;
    }
}
