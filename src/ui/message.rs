//! A message in a small box in the middle of the screen (tmux/say.sh), in
//! place of tmux's message on the status line, which is easy to miss and
//! gone in a moment: as wide as its text, wrapped to 72 columns, and as tall
//! as it; any key closes it, and the key goes no further; a click closes it
//! too; left alone, it closes itself after a few seconds.

use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;

use super::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use super::{Look, Outcome, Overlay, Step};
use crate::mux::input::{Key, Mouse};

/// How long a message stays up (say.sh's `-t`).
pub const SECONDS: u64 = 5;
/// The widest a message's text runs before it wraps.
const WRAP: usize = 72;

pub struct Message {
    lines: Vec<String>,
    until: Instant,
    close_lit: bool,
    drawn: Option<Drawn>,
}

impl Message {
    pub fn new(text: &str, seconds: u64) -> Message {
        Message {
            lines: draw::wrap(text.trim_end(), WRAP),
            until: Instant::now() + Duration::from_secs(seconds),
            close_lit: false,
            drawn: None,
        }
    }

    /// The text, as wrapped.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }
}

impl Overlay for Message {
    fn kind(&self) -> &'static str {
        "message"
    }

    fn stands_out(&self) -> Option<ratatui::layout::Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        // A blank row above the text, which is indented two columns, as
        // say.sh printed it; at least 30 columns wide.
        let lines: Vec<BoxLine> = std::iter::once(BoxLine::blank())
            .chain(
                self.lines
                    .iter()
                    .map(|l| BoxLine::text(format!("  {l}"), look.text())),
            )
            .collect();
        let mut spec = BoxSpec::new("", &lines);
        spec.width = 28;
        spec.close_lit = self.close_lit;
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, _key: Key, _look: &Look) -> Step {
        Step::Close(Outcome::Dismissed)
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        match draw::gesture(&mouse) {
            Gesture::Click => Step::Close(Outcome::Dismissed),
            Gesture::Move if look.hover => {
                let over = self
                    .drawn
                    .as_ref()
                    .is_some_and(|d| d.hit(mouse.col, mouse.row) == Hit::Close);
                if over == self.close_lit {
                    Step::Same
                } else {
                    self.close_lit = over;
                    Step::Redraw
                }
            }
            _ => Step::Same,
        }
    }

    fn deadline(&self) -> Option<Instant> {
        Some(self.until)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::input::{KeyCode, Mods, MouseButton, MouseKind};
    use crate::ui::draw::tests::screen;
    use ratatui::layout::Rect;

    #[test]
    fn it_is_as_wide_as_its_text_and_any_key_closes_it() {
        let mut m = Message::new("Nothing needs you.", SECONDS);
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        m.draw(&mut buf, &Look::default());
        let d = m.drawn.clone().unwrap();
        assert_eq!(d.outer.width, 30, "at least 30 columns");
        assert_eq!(d.outer.height, 4, "the edges, a blank row and the text");
        let s = screen(&buf);
        assert!(s[d.outer.y as usize + 2].contains("│  Nothing needs you."));
        assert_eq!(
            m.key(Key::new(KeyCode::Char('x'), Mods::NONE), &Look::default()),
            Step::Close(Outcome::Dismissed),
            "a key closes it and isn't passed on"
        );
        assert!(m.deadline().unwrap() > Instant::now() + Duration::from_secs(4));
    }

    #[test]
    fn long_text_wraps_and_a_click_closes_it() {
        let text = "word ".repeat(30);
        let mut m = Message::new(&text, SECONDS);
        assert!(m.lines().len() > 1);
        assert!(m.lines().iter().all(|l| l.len() <= 72));
        let click = Mouse {
            kind: MouseKind::Press,
            button: MouseButton::Left,
            col: 0,
            row: 0,
            mods: Mods::NONE,
        };
        assert_eq!(
            m.mouse(click, &Look::default()),
            Step::Close(Outcome::Dismissed)
        );
    }
}
