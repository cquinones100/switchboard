//! What every box with parts to move between keeps (the settings box, the
//! routine's box, the work item box, the remind box, the text box, the
//! views box and the key finder), so none keeps a copy of its own: which part has
//! Tab's focus and which is under the mouse, where each part was drawn on
//! the box's lines (whole lines, or a run of columns on one, as a button
//! is), what saving found wrong with a field, and the box's close with
//! what it holds as JSON, which whoever opened it reads back (`read`).
//!
//! A part is the box's own type (an enum of its fields and buttons, or an
//! index into a list), and its easy-mode × is one of them, `close`, which
//! the ring takes after the box's own stops while the look draws a ×.
//! Tab and Shift-Tab go round the stops a box gives (`Form::tab`), wrapping
//! at either end; a list's index moves round the same way (`step_index`),
//! which the answer box, the search box, the menu and the question step
//! their focus with.

use ratatui::layout::Rect;
use ratatui::style::Style;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::Look;
use crate::draw::{BoxLine, Drawn, Hit};
use crate::input::{Key, KeyCode, Mouse};
use crate::overlay::{Outcome, Step};
use crate::picker::Picked;
use crate::pointer::Pointer;

/// The stop `by` stops round from `at` in `stops`, wrapping; the first
/// when `at` isn't one.
pub fn step<P: Copy + PartialEq>(stops: &[P], at: P, by: isize) -> P {
    if stops.is_empty() {
        return at;
    }
    let from = stops.iter().position(|p| *p == at).unwrap_or(0) as isize;
    stops[(from + by).rem_euclid(stops.len() as isize) as usize]
}

/// The index `by` round from `at` among `n`, wrapping.
pub fn step_index(at: usize, n: usize, by: isize) -> usize {
    if n == 0 {
        return 0;
    }
    (at as isize + by).rem_euclid(n as isize) as usize
}

/// How far round a key moves Tab's focus: Tab one on, Shift-Tab one back.
pub fn tab_by(key: &Key) -> Option<isize> {
    match key.code {
        KeyCode::Tab if !key.mods.shift => Some(1),
        KeyCode::Tab | KeyCode::BackTab => Some(-1),
        _ => None,
    }
}

/// A box closing with `value` as JSON, `index` saying how (a box's Save
/// and Save and run, say).
pub fn close_with<T: Serialize>(index: usize, value: &T) -> Step {
    Step::Close(Outcome::Picked(Picked {
        index,
        value: serde_json::to_string(value).unwrap_or_default(),
        query: String::new(),
    }))
}

/// What a box closed with by `close_with`: how, and what it held.
pub fn read<T: DeserializeOwned>(outcome: &Outcome) -> Option<(usize, T)> {
    let Outcome::Picked(p) = outcome else {
        return None;
    };
    Some((p.index, serde_json::from_str(&p.value).ok()?))
}

/// A part's place on a box's lines: the line's index, its columns from the
/// inside's left edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Spot<P> {
    line: usize,
    from: u16,
    to: u16,
    part: P,
}

/// A box's parts: the focus, the one under the mouse, where each was drawn,
/// what is wrong, and where the box was drawn.
#[derive(Clone, Debug)]
pub struct Form<P> {
    /// The part with Tab's focus.
    pub focus: P,
    /// The part under the mouse.
    pub hover: Option<P>,
    /// The box's easy-mode ×, as a part.
    close: P,
    spots: Vec<Spot<P>>,
    /// What saving found wrong, by the part it is about.
    pub wrong: Option<(P, String)>,
    /// Where the box was last drawn.
    pub drawn: Option<Drawn>,
}

impl<P: Copy + PartialEq> Form<P> {
    /// A form whose focus starts on `focus`, its × the part `close`.
    pub fn new(focus: P, close: P) -> Form<P> {
        Form {
            focus,
            hover: None,
            close,
            spots: Vec::new(),
            wrong: None,
            drawn: None,
        }
    }

    /// Moves the focus `by` round `stops`, then the × where the look draws
    /// one.
    pub fn step(&mut self, stops: &[P], by: isize, look: &Look) {
        let mut stops = stops.to_vec();
        if look.close && !stops.contains(&self.close) {
            stops.push(self.close);
        }
        self.focus = step(&stops, self.focus, by);
    }

    /// Tab and Shift-Tab round `stops` (`step`); whether the key was one.
    pub fn tab(&mut self, key: &Key, stops: &[P], look: &Look) -> bool {
        match tab_by(key) {
            Some(by) => {
                self.step(stops, by, look);
                true
            }
            None => false,
        }
    }

    /// Whether a part has the focus or the mouse.
    pub fn lit(&self, part: P) -> bool {
        self.focus == part || self.hover == Some(part)
    }

    /// Whether the × is lit.
    pub fn close_lit(&self) -> bool {
        self.lit(self.close)
    }

    /// Forgets where the parts were drawn, as the box lays its lines out
    /// again.
    pub fn clear(&mut self) {
        self.spots.clear();
    }

    /// `part` is the whole of line `line`.
    pub fn line(&mut self, line: usize, part: P) {
        self.cols(line, 0, u16::MAX, part);
    }

    /// `part` is columns `from..to` of line `line`, counted from the
    /// inside's left edge.
    pub fn cols(&mut self, line: usize, from: u16, to: u16, part: P) {
        self.spots.push(Spot {
            line,
            from,
            to,
            part,
        });
    }

    /// The line `part` was drawn on.
    pub fn line_of(&self, part: P) -> Option<usize> {
        self.spots.iter().find(|s| s.part == part).map(|s| s.line)
    }

    /// The part at a spot of the box's lines: the narrowest there, so an
    /// option drawn over its field's line is the option.
    pub fn at_line(&self, line: usize, col: u16) -> Option<P> {
        self.spots
            .iter()
            .filter(|s| s.line == line && col >= s.from && col < s.to)
            .min_by_key(|s| s.to - s.from)
            .map(|s| s.part)
    }

    /// What a cell of the screen is in the box, and the part there (the ×
    /// for its button).
    pub fn hit(&self, col: u16, row: u16) -> (Hit, Option<P>) {
        let Some(drawn) = &self.drawn else {
            return (Hit::Outside, None);
        };
        let hit = drawn.hit(col, row);
        let part = match hit {
            Hit::Close => Some(self.close),
            Hit::Line(line, x) => self.at_line(line, x),
            _ => None,
        };
        (hit, part)
    }

    /// The mouse moved over `part` (or none): drawn again when the part
    /// under it changed.
    pub fn hover_on(&mut self, part: Option<P>) -> Step {
        if part == self.hover {
            Step::Same
        } else {
            self.hover = part;
            Step::Redraw
        }
    }

    /// The mouse moved to a cell, as `hover_on` the part there, while the
    /// look lights what is under it.
    pub fn hover(&mut self, mouse: &Mouse, look: &Look) -> Step {
        if !look.hover {
            return Step::Same;
        }
        let part = self.hit(mouse.col, mouse.row).1;
        self.hover_on(part)
    }

    /// The pointer over a cell: the hand over a part a click takes, else
    /// the arrow.
    pub fn pointer(&self, col: u16, row: u16, look: &Look) -> Option<Pointer> {
        let over = self.hit(col, row).1.is_some();
        Some(if over && look.mouse {
            Pointer::Hand
        } else {
            Pointer::Arrow
        })
    }

    /// Where the box stands out.
    pub fn outer(&self) -> Option<Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    /// Says what is wrong with `part`.
    pub fn wrong_at(&mut self, part: P, why: impl Into<String>) {
        self.focus = part;
        self.wrong = Some((part, why.into()));
    }

    /// What is wrong with `part`, if anything.
    pub fn wrong_of(&self, part: P) -> Option<&str> {
        self.wrong
            .as_ref()
            .filter(|(p, _)| *p == part)
            .map(|(_, w)| w.as_str())
    }

    /// Forgets what was wrong once the part it was about is typed in.
    pub fn typed_in(&mut self) {
        if self.wrong.as_ref().is_some_and(|(p, _)| *p == self.focus) {
            self.wrong = None;
        }
    }

    /// What is wrong with `part`, under it on `lines`, indented `indent`
    /// columns, in `style`.
    pub fn wrong_under(&self, part: P, lines: &mut Vec<BoxLine>, indent: usize, style: Style) {
        if let Some(why) = self.wrong_of(part) {
            lines.push(BoxLine::text(format!("{}{why}", " ".repeat(indent)), style));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Mods;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Part {
        A,
        B,
        Close,
    }

    #[test]
    fn tab_goes_round_the_stops_and_the_close_button_in_easy_mode() {
        let mut f = Form::new(Part::A, Part::Close);
        let tab = Key::new(KeyCode::Tab, Mods::NONE);
        let back = Key::new(KeyCode::BackTab, Mods::NONE);
        let plain = Look::default();
        assert!(f.tab(&tab, &[Part::A, Part::B], &plain));
        assert_eq!(f.focus, Part::B);
        f.tab(&tab, &[Part::A, Part::B], &plain);
        assert_eq!(f.focus, Part::A, "it wraps");
        let easy = Look {
            close: true,
            ..Look::default()
        };
        f.tab(&back, &[Part::A, Part::B], &easy);
        assert_eq!(f.focus, Part::Close, "the × after the stops");
        assert!(f.close_lit());
        assert!(!f.tab(&Key::new(KeyCode::Enter, Mods::NONE), &[Part::A], &easy));
        assert_eq!(step_index(0, 3, -1), 2);
        assert_eq!(step_index(2, 3, 1), 0);
    }

    #[test]
    fn a_part_is_found_by_its_line_and_columns() {
        let mut f = Form::new(Part::A, Part::Close);
        f.line(2, Part::A);
        f.cols(4, 3, 9, Part::B);
        assert_eq!(f.at_line(2, 40), Some(Part::A));
        assert_eq!(f.at_line(4, 5), Some(Part::B));
        assert_eq!(f.at_line(4, 9), None);
        assert_eq!(f.line_of(Part::B), Some(4));
        f.wrong_at(Part::B, "Give it a name.");
        assert_eq!(f.focus, Part::B);
        assert_eq!(f.wrong_of(Part::B), Some("Give it a name."));
        f.typed_in();
        assert_eq!(f.wrong, None);
    }

    #[test]
    fn what_a_box_closes_with_reads_back() {
        let step = close_with(1, &vec!["a".to_string()]);
        let Step::Close(outcome) = step else {
            panic!("it closes")
        };
        assert_eq!(read::<Vec<String>>(&outcome), Some((1, vec!["a".into()])));
        assert_eq!(read::<Vec<String>>(&Outcome::Dismissed), None);
    }
}
