//! A line to type in a box over mission control (render.js's `askModal`):
//! a tab's new name, a reminder's time or what it is for, one of your own
//! settings. Its hint dim above, then what is typed after a `›`; Enter takes
//! it (an empty line too, where that means something: unsetting a setting,
//! a reminder for nothing in particular), Esc goes back. With easy mode's
//! ×, Tab moves to it and Enter presses it; typing takes the focus back.
//!
//! It closes with what was typed as its outcome's value
//! (`Outcome::Picked`), trimmed.

use ratatui::buffer::Buffer;
use ratatui::text::Span;

use crate::mux::input::{Key, KeyCode, Mods, Mouse};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use crate::ui::{Look, Outcome, Overlay, Picked, Step};

pub struct Ask {
    title: String,
    hint: String,
    text: String,
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
            text: String::new(),
            empty: false,
            close_focus: false,
            drawn: None,
        }
    }

    /// What it starts with, to change rather than type again.
    pub fn text(mut self, text: impl Into<String>) -> Ask {
        self.text = text.into();
        self
    }

    /// Enter on an empty line takes it, rather than doing nothing.
    pub fn empty_ok(mut self) -> Ask {
        self.empty = true;
        self
    }

    /// What is typed now.
    pub fn typed(&self) -> &str {
        &self.text
    }
}

impl Overlay for Ask {
    fn kind(&self) -> &'static str {
        "ask"
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let inner = (buf.area.width as usize).saturating_sub(4).min(90);
        let mut lines: Vec<BoxLine> = draw::wrap(&self.hint, inner.saturating_sub(2))
            .into_iter()
            .map(|w| BoxLine::text(format!(" {w}"), look.dim()))
            .collect();
        lines.push(BoxLine::blank());
        lines.push(BoxLine::spans(vec![
            Span::styled(format!(" › {}", self.text), look.bright()),
            Span::styled("▏", look.dim()),
        ]));
        let mut spec = BoxSpec::new(&self.title, &lines);
        spec.keys = Some("type  ↵ done  esc back");
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
        match k.code {
            KeyCode::Esc => Step::Close(Outcome::Dismissed),
            KeyCode::Char('c') if k.mods == Mods::CTRL => Step::Close(Outcome::Dismissed),
            KeyCode::Enter => {
                let text = self.text.trim().to_string();
                if text.is_empty() && !self.empty {
                    return Step::Same;
                }
                Step::Close(Outcome::Picked(Picked {
                    index: 0,
                    value: text.clone(),
                    query: text,
                }))
            }
            KeyCode::Backspace => {
                self.text.pop();
                Step::Redraw
            }
            KeyCode::Char('u') if k.mods == Mods::CTRL => {
                self.text.clear();
                Step::Redraw
            }
            KeyCode::Char(c) if !k.mods.ctrl && !k.mods.alt => {
                self.text.push(c);
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
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse && matches!(hit, Hit::Close | Hit::Outside) => {
                Step::Close(Outcome::Dismissed)
            }
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
        self.text.push_str(&text.replace(['\n', '\r'], " "));
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
}
