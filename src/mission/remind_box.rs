//! The reminder's box (UX 2.0 board 34), in place of the list of times and
//! the "What for?" box after it: M-p's "Remind me about this tab", a tab's
//! menu and `e` in the Reminders view open it. "When" offers remind.sh's
//! five times, each with the clock time it means at the right, or one
//! typed, read back as it is typed ("✓ Tuesday, November 10 at 9:00 AM ·
//! in 3 days 19 hours", or what it couldn't read); "What for" holds the
//! note, the one set now to start with; under a rule, its keys as
//! keycaps (↵ Set it, M-x Clear it with one set, esc Cancel) and the
//! reminder set now.
//!
//! ↑ and ↓ choose a time and clear what was typed, typing in When takes
//! over the chosen time, Tab and Shift-Tab move between When, What for and
//! the keycaps, Enter sets it from either field (a time it can't read
//! keeping the box open), M-x clears it. Each time, field and keycap takes
//! a click, and the times light under the mouse.
//!
//! It closes with `Outcome::Picked`: index 0 to set it, its value the time
//! in seconds and its query the note; index 1 to clear it.

use jiff::Zoned;
use ratatui::buffer::Buffer;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use super::remind;
use crate::mux::input::{Key, KeyCode, Mods, Mouse};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use crate::ui::kit::{self, Surface, Tone};
use crate::ui::{Look, Outcome, Overlay, Picked, Step};

/// The box's inside, while the screen leaves room around it.
const WIDTH: usize = 66;

/// What Tab's focus or the mouse is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Time(usize),
    When,
    WhatFor,
    Set,
    Clear,
    Cancel,
    Close,
}

/// A part on a line: the line's index, its first and last column, the part.
type Placed = (usize, u16, u16, Part);

pub struct RemindBox {
    title: String,
    /// The time it reads times against, fixed for a test; `None` reads the
    /// clock each time, so a box left open never sets a time gone by.
    fixed: Option<Zoned>,
    chosen: usize,
    typed: String,
    note: String,
    /// The reminder set now: its time and note.
    set: Option<(i64, String)>,
    focus: Part,
    hover: Option<Part>,
    drawn: Option<Drawn>,
    /// Where each part of a line was drawn, by the line's index.
    parts: Vec<Placed>,
}

impl RemindBox {
    /// The box for the tab named `name` (its number and name), with the
    /// reminder set on it now, if any.
    pub fn new(name: &str, set: Option<(i64, String)>) -> RemindBox {
        let title = if set.is_some() {
            format!("Change the reminder for {name}")
        } else {
            format!("Remind me about {name}")
        };
        let note = set.as_ref().map(|s| s.1.clone()).unwrap_or_default();
        RemindBox {
            title,
            fixed: None,
            chosen: 0,
            typed: String::new(),
            note,
            set,
            focus: Part::When,
            hover: None,
            drawn: None,
            parts: Vec::new(),
        }
    }

    /// Times read as at `now` rather than the clock (the tests).
    #[cfg(test)]
    fn at_time(mut self, now: Zoned) -> RemindBox {
        self.fixed = Some(now);
        self
    }

    fn now(&self) -> Zoned {
        self.fixed.clone().unwrap_or_else(crate::util::local_now)
    }

    /// The times offered and what each means now.
    fn times(&self) -> Vec<(&'static str, Option<i64>)> {
        let now = self.now();
        remind::CHOICES
            .iter()
            .map(|c| (*c, remind::when(c, &now)))
            .collect()
    }

    /// What starts typed in When (a day chosen in the Reminders view).
    pub fn typed(mut self, typed: &str) -> RemindBox {
        self.typed = typed.to_string();
        self
    }

    /// The time it would set: what is typed, read, else the chosen time.
    fn at(&self) -> Option<i64> {
        if self.typed.trim().is_empty() {
            self.times().get(self.chosen).and_then(|t| t.1)
        } else {
            remind::when(self.typed.trim(), &self.now())
        }
    }

    fn stops(&self) -> Vec<Part> {
        let mut out = vec![Part::When, Part::WhatFor, Part::Set];
        if self.set.is_some() {
            out.push(Part::Clear);
        }
        out.push(Part::Cancel);
        out
    }

    fn step_focus(&mut self, by: isize, look: &Look) {
        let mut stops = self.stops();
        if look.close {
            stops.push(Part::Close);
        }
        let at = stops.iter().position(|p| *p == self.focus).unwrap_or(0) as isize;
        self.focus = stops[(at + by).rem_euclid(stops.len() as isize) as usize];
    }

    fn set_it(&self) -> Step {
        match self.at() {
            Some(at) => Step::Close(Outcome::Picked(Picked {
                index: 0,
                value: at.to_string(),
                query: self.note.trim().to_string(),
            })),
            // The read-back already says why.
            None => Step::Redraw,
        }
    }

    fn clear_it(&self) -> Step {
        if self.set.is_none() {
            return Step::Same;
        }
        Step::Close(Outcome::Picked(Picked {
            index: 1,
            value: String::new(),
            query: String::new(),
        }))
    }

    fn press(&mut self, part: Part) -> Step {
        match part {
            Part::Time(i) => {
                self.chosen = i;
                self.typed.clear();
                self.focus = Part::When;
                Step::Redraw
            }
            Part::When | Part::WhatFor => {
                self.focus = part;
                Step::Redraw
            }
            Part::Set => self.set_it(),
            Part::Clear => self.clear_it(),
            Part::Cancel | Part::Close => Step::Close(Outcome::Dismissed),
        }
    }

    fn field(&mut self) -> Option<&mut String> {
        match self.focus {
            Part::WhatFor => Some(&mut self.note),
            Part::When => Some(&mut self.typed),
            _ => None,
        }
    }

    /// The box's lines, and where each part on them is.
    fn lines(&self, inner: usize, look: &Look) -> (Vec<BoxLine>, Vec<Placed>) {
        let theme = &look.theme;
        let mut lines: Vec<BoxLine> = Vec::new();
        let mut parts = Vec::new();
        let label = |text: &str, on: bool| {
            let tone = if on { Tone::Accent } else { Tone::Dim };
            BoxLine::spans(kit::label(text, tone, None, inner, theme))
        };
        lines.push(label("When", self.focus == Part::When));
        let typing = !self.typed.trim().is_empty();
        let now = self.now();
        for (i, (text, at)) in self.times().into_iter().enumerate() {
            let chosen = !typing && i == self.chosen;
            let lit = self.hover == Some(Part::Time(i));
            let mut spans = kit::option(text, None, 0, chosen, theme);
            let clock = at.map(|a| remind::clock_of(a, &now)).unwrap_or_default();
            let gap = inner.saturating_sub(kit::spans_width(&spans) + draw::width(&clock) + 1);
            spans.push(Span::raw(" ".repeat(gap)));
            let tone = if chosen { Tone::Accent } else { Tone::Dim };
            spans.push(Span::styled(clock, tone.style(theme)));
            let fill = if chosen {
                Some(Surface::Chosen.color(theme))
            } else if lit {
                Some(Surface::Panel.color(theme))
            } else {
                None
            };
            parts.push((lines.len(), 0, inner as u16, Part::Time(i)));
            lines.push(BoxLine::spans(spans).fill(fill).clickable());
        }
        lines.push(BoxLine::text(" or type one", Tone::Dim.style(theme)));
        parts.push((lines.len(), 0, inner as u16, Part::When));
        lines.push(BoxLine::spans(field(
            &self.typed,
            "tue 9am, nov 20 3pm",
            inner,
            self.focus == Part::When,
            look,
        )));
        if typing {
            let back = match self.at() {
                Some(at) => vec![
                    Span::styled(" ✓ ", Tone::Done.bold(theme)),
                    Span::styled(remind::said_long(at, &now), Tone::Text.style(theme)),
                    Span::styled(
                        format!(" · {}", remind::left(at, now.timestamp().as_second())),
                        Tone::Dim.style(theme),
                    ),
                ],
                None => vec![Span::styled(
                    format!(" \"{}\" {}", self.typed.trim(), remind::NOT_A_TIME),
                    Tone::Needs.style(theme),
                )],
            };
            lines.push(BoxLine::spans(kit::fit(back, inner, Style::new())));
        }
        lines.push(BoxLine::text(
            " also: 45m, 2h, 3pm, 15:30, nov 20 3pm, 2027-01-04 9:00",
            Tone::Faint.style(theme),
        ));
        lines.push(BoxLine::blank());
        lines.push(label("What for", self.focus == Part::WhatFor));
        parts.push((lines.len(), 0, inner as u16, Part::WhatFor));
        lines.push(BoxLine::spans(field(
            &self.note,
            "what to pick up",
            inner,
            self.focus == Part::WhatFor,
            look,
        )));
        lines.push(BoxLine::text(
            " shown under the tab while it waits, and said when it comes due",
            Tone::Faint.style(theme),
        ));
        lines.push(BoxLine::spans(vec![kit::rule(inner, theme)]));
        let mut caps = vec![Span::raw(" ")];
        let mut cap = |key: &str, what: &str, part: Part, caps: &mut Vec<Span<'static>>| {
            let x = kit::spans_width(caps);
            let lit = self.focus == part || self.hover == Some(part);
            let spans = kit::keycap(key, what, lit, false, theme);
            let w = kit::spans_width(&spans);
            caps.extend(spans);
            parts.push((
                lines.len(),
                x as u16,
                (x + w).saturating_sub(2) as u16,
                part,
            ));
        };
        cap("↵", "Set it", Part::Set, &mut caps);
        if self.set.is_some() {
            cap("M-x", "Clear it", Part::Clear, &mut caps);
        }
        cap("esc", "Cancel", Part::Cancel, &mut caps);
        lines.push(BoxLine::spans(caps).clickable());
        if let Some((at, note)) = &self.set {
            let mut words = format!(" Set now: {}", remind::said_from(*at, &now));
            if !note.is_empty() {
                words.push_str(&format!(" · {note}"));
            }
            lines.push(BoxLine::text(
                draw::cut(&words, inner),
                Tone::Dim.style(theme),
            ));
        }
        (lines, parts)
    }
}

/// A field: the kit's input, its cursor only while it has the focus.
fn field(
    text: &str,
    placeholder: &str,
    inner: usize,
    focused: bool,
    look: &Look,
) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw(" ")];
    let input = kit::input(
        text,
        placeholder,
        inner.saturating_sub(2),
        &[],
        &look.theme,
        false,
    );
    spans.extend(input.into_iter().map(|s| {
        if !focused && s.content == "▏" {
            Span::styled(" ", s.style)
        } else if !focused && s.content.starts_with("› ") {
            Span::styled(
                s.content,
                Tone::Dim.style(&look.theme).add_modifier(Modifier::BOLD),
            )
        } else {
            s
        }
    }));
    spans
}

impl Overlay for RemindBox {
    fn kind(&self) -> &'static str {
        "remind"
    }

    fn stands_out(&self) -> Option<ratatui::layout::Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let inner = (buf.area.width as usize).saturating_sub(6).min(WIDTH);
        let (lines, parts) = self.lines(inner, look);
        self.parts = parts;
        let mut spec = BoxSpec::new(&self.title, &lines);
        spec.keys = Some("↑↓ a time  type your own  tab what for  ↵ set it  esc cancel");
        spec.width = inner as u16;
        spec.close_lit = self.focus == Part::Close || self.hover == Some(Part::Close);
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, look: &Look) -> Step {
        let k = key.normalized();
        match k.code {
            KeyCode::Esc => return Step::Close(Outcome::Dismissed),
            KeyCode::Char('c') if k.mods == Mods::CTRL => return Step::Close(Outcome::Dismissed),
            KeyCode::Char('x') if k.mods == Mods::ALT => return self.clear_it(),
            KeyCode::Tab => {
                self.step_focus(1, look);
                return Step::Redraw;
            }
            KeyCode::BackTab => {
                self.step_focus(-1, look);
                return Step::Redraw;
            }
            KeyCode::Up | KeyCode::Down => {
                let n = remind::CHOICES.len();
                let by = if k.code == KeyCode::Up { n - 1 } else { 1 };
                // Coming from a time typed, the first press keeps the one chosen.
                if self.typed.is_empty() {
                    self.chosen = (self.chosen + by) % n;
                }
                self.typed.clear();
                self.focus = Part::When;
                return Step::Redraw;
            }
            KeyCode::Enter => {
                return match self.focus {
                    Part::When | Part::WhatFor | Part::Time(_) => self.set_it(),
                    part => self.press(part),
                };
            }
            KeyCode::Char(' ')
                if matches!(
                    self.focus,
                    Part::Set | Part::Clear | Part::Cancel | Part::Close
                ) =>
            {
                return self.press(self.focus);
            }
            _ => {}
        }
        // Typing outside a field types in When.
        if self.field().is_none() {
            self.focus = Part::When;
        }
        let Some(text) = self.field() else {
            return Step::Same;
        };
        match k.code {
            KeyCode::Backspace => {
                text.pop();
                Step::Redraw
            }
            KeyCode::Char('u') if k.mods == Mods::CTRL => {
                text.clear();
                Step::Redraw
            }
            KeyCode::Char(c) if !k.mods.ctrl && !k.mods.alt => {
                text.push(c);
                Step::Redraw
            }
            _ => Step::Same,
        }
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        let hit = drawn.hit(mouse.col, mouse.row);
        let part = match hit {
            Hit::Close => Some(Part::Close),
            Hit::Line(line, x) => self
                .parts
                .iter()
                .find(|(l, x0, x1, _)| *l == line && x >= *x0 && x <= *x1)
                .map(|p| p.3),
            _ => None,
        };
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse => match part {
                Some(p) => self.press(p),
                None if hit == Hit::Outside => Step::Close(Outcome::Dismissed),
                None => Step::Same,
            },
            Gesture::Move if look.hover => {
                if part == self.hover {
                    Step::Same
                } else {
                    self.hover = part;
                    Step::Redraw
                }
            }
            _ => Step::Same,
        }
    }

    fn paste(&mut self, text: &str, _look: &Look) -> Step {
        if self.field().is_none() {
            self.focus = Part::When;
        }
        if let Some(field) = self.field() {
            field.push_str(&text.replace(['\n', '\r'], " "));
        }
        Step::Redraw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> Key {
        Key::new(code, Mods::NONE)
    }

    fn now() -> Zoned {
        "2026-11-04T14:30:00[America/New_York]".parse().unwrap()
    }

    fn text_of(b: &RemindBox) -> String {
        let (lines, _) = b.lines(WIDTH, &Look::default());
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_times_carry_their_clock_and_a_typed_one_is_read_back() {
        let mut b = RemindBox::new("3 api", None).at_time(now());
        let text = text_of(&b);
        assert!(text.contains("In 20 minutes"), "{text}");
        assert!(text.contains("2:50"), "{text}");
        assert!(text.contains("Thu 9:00"), "{text}");
        assert!(!text.contains("Set now"), "{text}");
        assert!(!text.contains("Clear it"), "{text}");
        for c in "tue 9am".chars() {
            b.key(key(KeyCode::Char(c)), &Look::default());
        }
        let text = text_of(&b);
        assert!(
            text.contains("✓ Tuesday, November 10 at 9:00 AM · in 5 days 18 hours"),
            "{text}"
        );
        b.key(key(KeyCode::Char('x')), &Look::default());
        let text = text_of(&b);
        assert!(text.contains("isn't a time I know"), "{text}");
        assert_eq!(b.key(key(KeyCode::Enter), &Look::default()), Step::Redraw);
    }

    #[test]
    fn down_down_tab_a_note_and_enter_set_it_three_hours_on() {
        let n = now();
        let mut b = RemindBox::new(
            "3 api",
            Some((n.timestamp().as_second() + 60, "old".into())),
        )
        .at_time(n.clone());
        let text = text_of(&b);
        assert!(
            text.contains("Change the reminder") || b.title.contains("Change"),
            "{text}"
        );
        assert!(text.contains("Set now:"), "{text}");
        assert!(text.contains("Clear it"), "{text}");
        let look = Look::default();
        b.key(key(KeyCode::Down), &look);
        b.key(key(KeyCode::Down), &look);
        b.key(key(KeyCode::Tab), &look);
        for _ in 0..3 {
            b.key(key(KeyCode::Backspace), &look);
        }
        for c in "the handoff".chars() {
            b.key(key(KeyCode::Char(c)), &look);
        }
        match b.key(key(KeyCode::Enter), &look) {
            Step::Close(Outcome::Picked(p)) => {
                assert_eq!(p.index, 0);
                assert_eq!(p.value, (n.timestamp().as_second() + 3 * 3600).to_string());
                assert_eq!(p.query, "the handoff");
            }
            other => panic!("{other:?}"),
        }
        let alt_x = Key::new(KeyCode::Char('x'), Mods::ALT);
        assert!(matches!(
            b.key(alt_x, &look),
            Step::Close(Outcome::Picked(p)) if p.index == 1
        ));
    }
}
