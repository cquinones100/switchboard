//! A line to type in a box over mission control (render.js's `askModal`):
//! a tab's new name, a reminder's time or what it is for, one of your own
//! settings. Its hint dim above, then what is typed after a `›`; Enter takes
//! it (an empty line too, where that means something: unsetting a setting,
//! a reminder for nothing in particular), Esc goes back. With easy mode's
//! ×, Tab moves to it and Enter presses it; typing takes the focus back.
//!
//! What is typed is a `ui::field`, with its cursor keys, wrapping at the
//! box's width and a click putting the cursor; one whose use takes more
//! than a line (`multi_line`) takes S-Enter and M-Enter as a new line.
//!
//! It closes with what was typed as its outcome's value
//! (`Outcome::Picked`), trimmed.

use ratatui::buffer::Buffer;
use ratatui::text::Span;

use crate::mux::input::{Key, KeyCode, Mods, Mouse};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use crate::ui::field::{Edit, Field};
use crate::ui::{Look, Outcome, Overlay, Picked, Step};

pub struct Ask {
    title: String,
    hint: String,
    field: Field,
    /// The index of the field's first row among the box's lines.
    field_line: usize,
    /// Whether Enter on an empty line takes it.
    empty: bool,
    close_focus: bool,
    drawn: Option<Drawn>,
}

impl Ask {
    pub fn new(title: impl Into<String>, hint: impl Into<String>) -> Ask {
        Ask {
            title: title.into(),
            hint: hint.into(),
            field: Field::new(),
            field_line: 0,
            empty: false,
            close_focus: false,
            drawn: None,
        }
    }

    /// What it starts with, to change rather than type again.
    pub fn text(mut self, text: impl Into<String>) -> Ask {
        let text = text.into();
        // Text of several lines (a shell command kept in a setting) keeps
        // them, and takes S-Enter for another.
        let field = if text.contains(['\n', '\r']) || self.field.is_multi_line() {
            Field::new().multi_line()
        } else {
            Field::new()
        };
        self.field = field.with_text(text);
        self
    }

    /// It takes more than a line: S-Enter and M-Enter add one.
    pub fn multi_line(mut self) -> Ask {
        self.field = Field::new().multi_line().with_text(self.field.text());
        self
    }

    /// Enter on an empty line takes it, rather than doing nothing.
    pub fn empty_ok(mut self) -> Ask {
        self.empty = true;
        self
    }

    /// What is typed now.
    pub fn typed(&self) -> &str {
        self.field.text()
    }
}

impl Overlay for Ask {
    fn drawn(&self) -> Option<&crate::ui::draw::Drawn> {
        self.drawn.as_ref()
    }

    fn kind(&self) -> &'static str {
        "ask"
    }

    fn stands_out(&self) -> Option<ratatui::layout::Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let inner = (buf.area.width as usize).saturating_sub(4).min(90);
        let mut lines: Vec<BoxLine> = draw::wrap(&self.hint, inner.saturating_sub(2))
            .into_iter()
            .map(|w| BoxLine::text(format!(" {w}"), look.dim()))
            .collect();
        lines.push(BoxLine::blank());
        self.field_line = lines.len();
        // Up to half the screen, and never more than the box has rows for
        // under the hint.
        let left = (buf.area.height as usize).saturating_sub(6 + lines.len());
        let most = (buf.area.height as usize / 2)
            .saturating_sub(4)
            .min(left)
            .max(1);
        let rows = self.field.view(
            inner.saturating_sub(4),
            most,
            look.bright(),
            look.dim(),
            true,
        );
        for (i, row) in rows.into_iter().enumerate() {
            let lead = if i == 0 { " › " } else { "   " };
            let mut spans = vec![Span::styled(lead, look.bright())];
            spans.extend(row);
            lines.push(BoxLine::spans(spans));
        }
        let mut spec = BoxSpec::new(&self.title, &lines);
        spec.keys = Some(if self.field.is_multi_line() {
            "type  ⇧↵ new line  ↵ done  esc back"
        } else {
            "type  ↵ done  esc back"
        });
        spec.width = inner as u16;
        spec.close_lit = self.close_focus;
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, look: &Look) -> Step {
        let k = key.normalized();
        if matches!(k.code, KeyCode::Tab | KeyCode::BackTab) && look.close {
            self.close_focus = !self.close_focus;
            return Step::Redraw;
        }
        if self.close_focus && matches!(k.code, KeyCode::Enter | KeyCode::Char(' ')) {
            return Step::Close(Outcome::Dismissed);
        }
        self.close_focus = false;
        match self.field.key(k) {
            Edit::Changed | Edit::Moved => return Step::Redraw,
            Edit::Same | Edit::Up | Edit::Down => return Step::Same,
            Edit::Not => {}
        }
        match k.code {
            KeyCode::Esc => Step::Close(Outcome::Dismissed),
            KeyCode::Char('c') if k.mods == Mods::CTRL => Step::Close(Outcome::Dismissed),
            KeyCode::Enter => {
                let text = self.field.text().trim().to_string();
                if text.is_empty() && !self.empty {
                    return Step::Same;
                }
                Step::Close(Outcome::Picked(Picked {
                    index: 0,
                    value: text.clone(),
                    query: text,
                }))
            }
            _ => Step::Same,
        }
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        let hit = drawn.hit(mouse.col, mouse.row);
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse && matches!(hit, Hit::Close | Hit::Outside) => {
                Step::Close(Outcome::Dismissed)
            }
            Gesture::Click if look.mouse => match hit {
                Hit::Line(i, col) if i >= self.field_line => {
                    self.close_focus = false;
                    self.field
                        .click(i - self.field_line, (col as usize).saturating_sub(3));
                    Step::Redraw
                }
                _ => Step::Same,
            },
            Gesture::Move if look.hover => {
                let close = hit == Hit::Close;
                if close == self.close_focus {
                    Step::Same
                } else {
                    self.close_focus = close;
                    Step::Redraw
                }
            }
            _ => Step::Same,
        }
    }

    fn paste(&mut self, text: &str, _look: &Look) -> Step {
        self.field.paste(text);
        Step::Redraw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> Key {
        Key::new(code, Mods::NONE)
    }

    #[test]
    fn it_takes_what_is_typed_and_an_empty_line_only_when_that_means_something() {
        let look = Look::default();
        let mut a = Ask::new("Rename", "A name.").text("old");
        for _ in 0..3 {
            a.key(key(KeyCode::Backspace), &look);
        }
        assert_eq!(a.key(key(KeyCode::Enter), &look), Step::Same);
        for c in "new one ".chars() {
            a.key(key(KeyCode::Char(c)), &look);
        }
        assert!(matches!(
            a.key(key(KeyCode::Enter), &look),
            Step::Close(Outcome::Picked(p)) if p.value == "new one"
        ));
        let mut a = Ask::new("What for?", "Optional.").empty_ok();
        assert!(matches!(
            a.key(key(KeyCode::Enter), &look),
            Step::Close(Outcome::Picked(p)) if p.value.is_empty()
        ));
        let mut a = Ask::new("x", "y");
        assert_eq!(
            a.key(key(KeyCode::Esc), &look),
            Step::Close(Outcome::Dismissed)
        );
    }

    fn screen(a: &mut Ask, cols: u16, rows: u16) -> Vec<String> {
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, rows));
        a.draw(&mut buf, &Look::default());
        crate::ui::draw::tests::screen(&buf)
    }

    #[test]
    fn s_enter_adds_a_line_in_a_multi_line_ask_and_enter_takes_both() {
        let look = Look::default();
        let mut a = Ask::new("Shape a new ticket", "What about?").multi_line();
        a.paste("first", &look);
        assert_eq!(
            a.key(Key::new(KeyCode::Enter, Mods::SHIFT), &look),
            Step::Redraw
        );
        a.paste("second", &look);
        let s = screen(&mut a, 60, 20);
        assert!(s.iter().any(|l| l.contains("› first")), "{s:#?}");
        assert!(s.iter().any(|l| l.contains("   second▏")), "{s:#?}");
        assert!(matches!(
            a.key(key(KeyCode::Enter), &look),
            Step::Close(Outcome::Picked(p)) if p.value == "first\nsecond"
        ));
        let mut one = Ask::new("Rename", "A name.").text("a");
        assert_eq!(
            one.key(Key::new(KeyCode::Enter, Mods::SHIFT), &look),
            Step::Same
        );
    }

    #[test]
    fn text_of_several_lines_keeps_them() {
        let look = Look::default();
        let mut a = Ask::new("Test command", "A command.").text("cat <<-EOF\n\tx\n\tEOF");
        assert!(matches!(
            a.key(key(KeyCode::Enter), &look),
            Step::Close(Outcome::Picked(p)) if p.value == "cat <<-EOF\n\tx\n\tEOF"
        ));
    }

    #[test]
    fn a_long_name_wraps_in_the_box() {
        let mut a = Ask::new("Rename", "A name.").text("alpha bravo charlie delta echo foxtrot");
        let s = screen(&mut a, 24, 20);
        assert!(s.iter().any(|l| l.contains("› alpha bravo")), "{s:#?}");
        assert!(s.iter().any(|l| l.contains("foxtrot▏")), "{s:#?}");
    }
}
