//! The text box a box over mission control types into, with its buttons
//! (UX 2.0 board 108's reply box, which it came out of): a field drawn on
//! the panel, five rows at least and growing with the text to what the box
//! gives it, scrolled past that to keep the cursor in sight, then a row
//! with a faint note at the left and Cancel and the box's own button at
//! the right. Tab moves through the text, the button, Cancel and easy
//! mode's ×; Enter on the text, or Enter or Space on the button, sends;
//! S-Enter and M-Enter are the field's new lines; Esc cancels; typing on a
//! button types in the text. The Slack reply box and the multi-line ask
//! box (Shape a new ticket) draw with it, each keeping the lines it has
//! around it. New, with no tmux-version file behind it.

use ratatui::text::Span;

use crate::mux::input::{Key, KeyCode, Mods, Mouse};
use crate::ui::draw::{self, BoxLine, Drawn, Gesture, Hit};
use crate::ui::field::{Edit, Field};
use crate::ui::kit::{self, Press, Surface, Tone};
use crate::ui::{Look, Outcome, Picked, Step};

/// The rows the text takes at least.
pub const LEAST_ROWS: usize = 5;

/// What Tab's focus or the mouse is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Text,
    Send,
    Cancel,
    Close,
}

/// A part on a line: the line's index, its first and last column, the part.
type Placed = (usize, u16, u16, Part);

pub struct TextBox {
    pub field: Field,
    /// What the empty text shows, faint.
    placeholder: String,
    /// The button that sends.
    send: String,
    focus: Part,
    hover: Option<Part>,
    /// The box's lines that hold the field's rows.
    field_lines: std::ops::Range<usize>,
    parts: Vec<Placed>,
}

impl TextBox {
    /// A text box holding `field`, `placeholder` shown while it is empty,
    /// its button named `send`.
    pub fn new(field: Field, placeholder: &str, send: &str) -> TextBox {
        TextBox {
            field,
            placeholder: placeholder.to_string(),
            send: send.to_string(),
            focus: Part::Text,
            hover: None,
            field_lines: 0..0,
            parts: Vec::new(),
        }
    }

    /// Whether easy mode's × is lit.
    pub fn close_lit(&self) -> bool {
        self.focus == Part::Close || self.hover == Some(Part::Close)
    }

    /// The rows the text gets in a box whose screen is `height` rows tall
    /// and has `around` rows of its own besides: half the screen at most,
    /// what is left at most, and `LEAST_ROWS` at least.
    pub fn most_rows(height: usize, around: usize) -> usize {
        (height / 2)
            .min(height.saturating_sub(around))
            .max(LEAST_ROWS)
    }

    /// The text's rows, added to `lines`, `inner` wide, `most` rows at
    /// most, then a blank line.
    pub fn text_lines(&mut self, lines: &mut Vec<BoxLine>, inner: usize, most: usize, look: &Look) {
        self.parts.clear();
        let theme = &look.theme;
        let area = inner.saturating_sub(4);
        let panel = Surface::Panel.color(theme);
        let typing = self.focus == Part::Text;
        let on_panel = |tone: Tone| tone.style(theme).bg(panel);
        let room = area.saturating_sub(2);
        let taken = self.field.rows(room).len();
        let shown = taken.clamp(LEAST_ROWS, most.max(LEAST_ROWS));
        let rows = self.field.view(
            room,
            shown,
            on_panel(Tone::Text),
            on_panel(Tone::Accent),
            typing,
        );
        let first = lines.len();
        for i in 0..shown {
            let mut spans = vec![Span::raw(" "), Span::styled(" ", on_panel(Tone::Text))];
            if let Some(row) = rows.get(i) {
                spans.extend(row.iter().cloned());
            }
            if self.field.is_empty() && i == 0 {
                spans.push(Span::styled(
                    self.placeholder.clone(),
                    on_panel(Tone::Faint),
                ));
            }
            let used = kit::spans_width(&spans) - 1;
            spans.push(Span::styled(
                " ".repeat(area.saturating_sub(used)),
                on_panel(Tone::Text),
            ));
            self.parts.push((lines.len(), 1, area as u16, Part::Text));
            lines.push(BoxLine::spans(spans).clickable());
        }
        self.field_lines = first..lines.len();
        lines.push(BoxLine::blank());
    }

    /// The buttons' row, added to `lines`: `note` faint at the left, then
    /// Cancel and the button that sends at the right.
    pub fn buttons_line(
        &mut self,
        lines: &mut Vec<BoxLine>,
        inner: usize,
        note: &str,
        look: &Look,
    ) {
        let theme = &look.theme;
        let press = |part: Part| {
            if self.focus == part {
                Press::Lit
            } else if self.hover == Some(part) {
                Press::Hover
            } else {
                Press::Rest
            }
        };
        let cancel = kit::button(" Cancel ", None, press(Part::Cancel), false, theme);
        let send = kit::button(
            &format!(" {} ", self.send),
            None,
            press(Part::Send),
            false,
            theme,
        );
        let (cw, sw) = (kit::spans_width(&cancel), kit::spans_width(&send));
        let gap = inner.saturating_sub(draw::width(note) + cw + sw + 3);
        let mut row = vec![
            Span::styled(note.to_string(), Tone::Faint.style(theme)),
            Span::raw(" ".repeat(gap)),
        ];
        let cancel_at = kit::spans_width(&row);
        row.extend(cancel);
        row.push(Span::raw(" "));
        let send_at = kit::spans_width(&row);
        row.extend(send);
        self.parts.push((
            lines.len(),
            cancel_at as u16,
            (cancel_at + cw) as u16,
            Part::Cancel,
        ));
        self.parts.push((
            lines.len(),
            send_at as u16,
            (send_at + sw) as u16,
            Part::Send,
        ));
        lines.push(BoxLine::spans(row).clickable());
    }

    fn stops(look: &Look) -> Vec<Part> {
        let mut out = vec![Part::Text, Part::Send, Part::Cancel];
        if look.close {
            out.push(Part::Close);
        }
        out
    }

    fn step_focus(&mut self, by: isize, look: &Look) {
        let stops = Self::stops(look);
        let at = stops.iter().position(|p| *p == self.focus).unwrap_or(0) as isize;
        self.focus = stops[(at + by).rem_euclid(stops.len() as isize) as usize];
    }

    /// A part pressed: the button sends what is typed (`Outcome::Picked`,
    /// its query the text as typed), unless nothing is.
    fn press(&mut self, part: Part) -> Step {
        match part {
            Part::Text => {
                self.focus = Part::Text;
                Step::Redraw
            }
            // Nothing typed, nothing to send: the box stays.
            Part::Send if self.field.text().trim().is_empty() => {
                self.focus = Part::Text;
                Step::Redraw
            }
            Part::Send => Step::Close(Outcome::Picked(Picked {
                index: 0,
                value: String::new(),
                query: self.field.text().to_string(),
            })),
            Part::Cancel | Part::Close => Step::Close(Outcome::Dismissed),
        }
    }

    pub fn key(&mut self, key: Key, look: &Look) -> Step {
        let k = key.normalized();
        match k.code {
            KeyCode::Esc => return Step::Close(Outcome::Dismissed),
            KeyCode::Char('c') if k.mods == Mods::CTRL => return Step::Close(Outcome::Dismissed),
            KeyCode::Tab => {
                self.step_focus(1, look);
                return Step::Redraw;
            }
            KeyCode::BackTab => {
                self.step_focus(-1, look);
                return Step::Redraw;
            }
            KeyCode::Enter if self.focus != Part::Text => return self.press(self.focus),
            KeyCode::Char(' ') if self.focus != Part::Text => return self.press(self.focus),
            _ => {}
        }
        // Typing on a button types in the text.
        self.focus = Part::Text;
        match self.field.key(k) {
            // Enter on the text sends; S-Enter and M-Enter are the field's
            // new lines.
            Edit::Not if k.code == KeyCode::Enter => self.press(Part::Send),
            Edit::Changed | Edit::Moved => Step::Redraw,
            Edit::Same | Edit::Up | Edit::Down | Edit::Not => Step::Same,
        }
    }

    pub fn mouse(&mut self, mouse: Mouse, drawn: Option<&Drawn>, look: &Look) -> Step {
        let Some(drawn) = drawn else {
            return Step::Same;
        };
        let hit = drawn.hit(mouse.col, mouse.row);
        let part = match hit {
            Hit::Close => Some(Part::Close),
            Hit::Line(line, x) => self
                .parts
                .iter()
                .find(|(l, x0, x1, _)| *l == line && x >= *x0 && x < *x1)
                .map(|p| p.3),
            _ => None,
        };
        match draw::gesture(&mouse) {
            // A click outside keeps what was typed rather than losing it.
            Gesture::Click if look.mouse => match (hit, part) {
                (Hit::Line(line, x), Some(Part::Text)) if self.field_lines.contains(&line) => {
                    self.focus = Part::Text;
                    self.field.click(
                        line - self.field_lines.start,
                        (x as usize).saturating_sub(2),
                    );
                    Step::Redraw
                }
                (_, part) => part.map_or(Step::Same, |p| self.press(p)),
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

    pub fn paste(&mut self, text: &str) -> Step {
        self.focus = Part::Text;
        self.field.paste(text);
        Step::Redraw
    }
}
