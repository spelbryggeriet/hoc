const LENGTH: usize = 8;
const SLOWDOWN: usize = 4;

const BRAILLE_SPIN_ANIMATION: [char; LENGTH] = ['⢹', '⣸', '⣴', '⣦', '⣇', '⡏', '⠟', '⠻'];
const BOX_SIDE_SWELL_ANIMATION: [char; LENGTH] = ['│', '╿', '┃', '┃', '┃', '┃', '╽', '│'];
const BOX_TURN_SWELL_ANIMATION: [char; LENGTH] = ['└', '┖', '┗', '┗', '┗', '┗', '┕', '└'];
const BOX_END_SWELL_ANIMATION: [char; LENGTH] = ['╴', '╸', '╸', '╸', '╸', '╸', '╴', '╴'];
const SEPARATOR_SWELL_ANIMATION: [char; LENGTH] = ['─', '─', '─', '─', '─', '─', '─', '─'];

const BRAILLE_SPIN_PAUSED: char = '';
const BOX_SIDE_SWELL_PAUSED: char = '│';
const BOX_TURN_SWELL_PAUSED: char = '└';
const BOX_END_SWELL_PAUSED: char = '╴';
const SEPARATOR_SWELL_PAUSED: char = '─';

const BRAILLE_SPIN_FINISHED: char = '';
const BOX_SIDE_SWELL_FINISHED: char = '┃';
const BOX_TURN_SWELL_FINISHED: char = '┗';
const BOX_END_SWELL_FINISHED: char = '╸';
const SEPARATOR_SWELL_FINISHED: char = '━';

#[derive(Copy, Clone)]
pub enum State {
    Animating(isize),
    Finished,
    Paused,
}

impl State {
    pub fn frame_offset(self, offset: isize) -> Self {
        if let Self::Animating(frame) = self {
            Self::Animating(frame + offset)
        } else {
            self
        }
    }
}

pub fn braille_spin(state: State) -> char {
    animate(
        state,
        BRAILLE_SPIN_ANIMATION,
        BRAILLE_SPIN_PAUSED,
        BRAILLE_SPIN_FINISHED,
    )
}

pub fn box_side_swell(state: State) -> char {
    animate(
        state,
        BOX_SIDE_SWELL_ANIMATION,
        BOX_SIDE_SWELL_PAUSED,
        BOX_SIDE_SWELL_FINISHED,
    )
}

pub fn box_turn_swell(state: State) -> char {
    animate(
        state,
        BOX_TURN_SWELL_ANIMATION,
        BOX_TURN_SWELL_PAUSED,
        BOX_TURN_SWELL_FINISHED,
    )
}

pub fn box_end_swell(state: State) -> char {
    animate(
        state,
        BOX_END_SWELL_ANIMATION,
        BOX_END_SWELL_PAUSED,
        BOX_END_SWELL_FINISHED,
    )
}

pub fn separator_swell(state: State) -> char {
    animate(
        state,
        SEPARATOR_SWELL_ANIMATION,
        SEPARATOR_SWELL_PAUSED,
        SEPARATOR_SWELL_FINISHED,
    )
}

fn animate(
    state: State,
    animation_chars: [char; LENGTH],
    paused_char: char,
    finished_char: char,
) -> char {
    match state {
        State::Animating(frame) => {
            let index = frame.rem_euclid(LENGTH as isize) as usize;
            animation_chars[index]
        }
        State::Paused => paused_char,
        State::Finished => finished_char,
    }
}

pub struct Frames {
    frame_index: usize,
    slowdown_index: usize,
}

impl Frames {
    pub fn new() -> Self {
        Self {
            frame_index: LENGTH - 1,
            slowdown_index: SLOWDOWN - 1,
        }
    }
}

impl Iterator for Frames {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        self.frame_index = (self.frame_index + (self.slowdown_index + 1) / SLOWDOWN) % LENGTH;
        self.slowdown_index = (self.slowdown_index + 1) % SLOWDOWN;
        Some(self.frame_index)
    }
}
