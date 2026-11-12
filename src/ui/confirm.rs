//! A question before something that can't be undone (tmux/confirm.sh and
//! mission control's `confirmModal`): the question in yellow and bold with
//! `[Y/n]`, anything it would act on listed under it, and Yes and No on a
//! line of their own. Enter takes the one with the `›`, Yes until Tab moves
//! it; y says yes; Esc, n, a click outside or any other key says no. Space
//! presses the chosen one only once Tab has moved, so a stray Space is
//! still no. Easy mode's × is a Tab stop after No, pressed as no.

use ratatui::buffer::Buffer;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use super::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use super::{Look, Outcome, Overlay, Step};
use crate::mux::input::{Key, KeyCode, Mouse};

/// Which of the question's buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button {
    Yes,
    No,
    Close,
}

pub struct Confirm {
    text: String,
    list: Vec<String>,
    /// What Enter takes now.
    chosen: Button,
    /// Tab has moved the choice, so Space presses it.
    tabbed: bool,
    /// Under the mouse.
    lit: Option<Button>,
    drawn: Option<Drawn>,
    /// Where it opens: the middle, or beside the click that opened it.
    place: draw::Place,
    /// The columns of Yes and No on their line, inside the box, and which
    /// of the box's lines that is.
    spans: [(u16, u16); 2],
    buttons_at: usize,
}

impl Confirm {
    pub fn new(text: impl Into<String>) -> Confirm {
        Confirm {
            text: text.into(),
            list: Vec::new(),
            chosen: Button::Yes,
            tabbed: false,
            lit: None,
            drawn: None,
            place: draw::Place::Center,
            spans: [(0, 0); 2],
            buttons_at: 0,
        }
    }

    /// What it would act on, a line each, under the question.
    pub fn list(mut self, items: Vec<String>) -> Confirm {
        self.list = items;
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    fn answer(button: Button) -> Step {
        Step::Close(match button {
            Button::Yes => Outcome::Yes,
            Button::No | Button::Close => Outcome::No,
        })
    }

    /// Yes, No, then the close button when there is one, round in either
    /// direction.
    fn tab(&mut self, back: bool, look: &Look) {
        let stops: &[Button] = if look.close {
            &[Button::Yes, Button::No, Button::Close]
        } else {
            &[Button::Yes, Button::No]
        };
        let at = stops.iter().position(|b| *b == self.chosen).unwrap_or(0);
        let n = stops.len();
        self.chosen = stops[if back { (at + n - 1) % n } else { (at + 1) % n }];
        self.tabbed = true;
    }
}

impl Overlay for Confirm {
    fn kind(&self) -> &'static str {
        "confirm"
    }

    fn stands_out(&self) -> Option<ratatui::layout::Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let question = Style::new().fg(draw::YELLOW).add_modifier(Modifier::BOLD);
        let mut lines = vec![BoxLine::text(format!(" {} [Y/n]", self.text), question)];
        if !self.list.is_empty() {
            lines.push(BoxLine::blank());
            let room = buf.area.width.saturating_sub(12) as usize;
            for item in &self.list {
                lines.push(BoxLine::text(
                    format!("   {}", draw::cut(item, room)),
                    look.text(),
                ));
            }
        }
        lines.push(BoxLine::blank());
        // The one Enter takes has the › and is bright; the one under the
        // mouse is in reverse video, which Enter doesn't take.
        let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
        let mut at = 1u16;
        for (i, (button, label)) in [(Button::Yes, "Yes"), (Button::No, "No")]
            .into_iter()
            .enumerate()
        {
            if i > 0 {
                spans.push(Span::raw("  "));
                at += 2;
            }
            let start = at;
            if self.chosen == button {
                spans.push(look.pointer());
                spans.push(Span::raw(" "));
            } else {
                spans.push(Span::raw("  "));
            }
            let mut style = if self.chosen == button {
                look.bold_bright()
            } else {
                look.text()
            };
            if self.lit == Some(button) {
                style = style.add_modifier(Modifier::REVERSED);
            }
            spans.push(Span::styled(label, style));
            at += 2 + label.len() as u16;
            self.spans[i] = (start, at);
        }
        self.buttons_at = lines.len();
        lines.push(BoxLine::spans(spans).clickable());
        let mut spec = BoxSpec::new("Are you sure?", &lines);
        spec.keys = Some("y or click Yes  tab move  ↵ choose  any other key no");
        spec.edge = draw::YELLOW;
        spec.close_lit = self.chosen == Button::Close || self.lit == Some(Button::Close);
        spec.place = self.place;
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn open_near(&mut self, (x, y): (u16, u16)) {
        self.place = draw::Place::Near { x, y };
    }

    fn key(&mut self, key: Key, look: &Look) -> Step {
        let plain = key.mods.is_empty() || key.code == KeyCode::BackTab;
        match key.code {
            KeyCode::Enter if plain => Confirm::answer(self.chosen),
            KeyCode::Char('y' | 'Y') if !key.mods.ctrl && !key.mods.alt => {
                Step::Close(Outcome::Yes)
            }
            KeyCode::Char(' ') if plain => {
                if self.tabbed {
                    Confirm::answer(self.chosen)
                } else {
                    Step::Close(Outcome::No)
                }
            }
            KeyCode::Tab if plain => {
                self.tab(false, look);
                Step::Redraw
            }
            KeyCode::BackTab => {
                self.tab(true, look);
                Step::Redraw
            }
            _ => Step::Close(Outcome::No),
        }
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        let under = match drawn.hit(mouse.col, mouse.row) {
            Hit::Close => Some(Button::Close),
            Hit::Line(i, col) if i == self.buttons_at => {
                if col >= self.spans[0].0 && col < self.spans[0].1 {
                    Some(Button::Yes)
                } else if col >= self.spans[1].0 && col < self.spans[1].1 {
                    Some(Button::No)
                } else {
                    None
                }
            }
            Hit::Outside => {
                if draw::gesture(&mouse) == Gesture::Click && look.mouse {
                    return Step::Close(Outcome::No);
                }
                None
            }
            _ => None,
        };
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse => match under {
                Some(b) => Confirm::answer(b),
                None => Step::Same,
            },
            Gesture::Move if look.hover => {
                if under == self.lit {
                    Step::Same
                } else {
                    self.lit = under;
                    Step::Redraw
                }
            }
            _ => Step::Same,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::input::{Mods, MouseButton, MouseKind};
    use crate::ui::draw::tests::screen;
    use ratatui::layout::Rect;

    fn key(code: KeyCode) -> Key {
        Key::new(code, Mods::NONE)
    }

    fn drawn(c: &mut Confirm, look: &Look) -> Vec<String> {
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 20));
        c.draw(&mut buf, look);
        screen(&buf)
    }

    #[test]
    fn enter_is_yes_and_anything_else_no() {
        let look = Look::default();
        assert_eq!(
            Confirm::new("Close pane?").key(key(KeyCode::Enter), &look),
            Step::Close(Outcome::Yes)
        );
        assert_eq!(
            Confirm::new("x").key(key(KeyCode::Char('y')), &look),
            Step::Close(Outcome::Yes)
        );
        for code in [KeyCode::Esc, KeyCode::Char('n'), KeyCode::Char('q')] {
            assert_eq!(
                Confirm::new("x").key(key(code), &look),
                Step::Close(Outcome::No)
            );
        }
        assert_eq!(
            Confirm::new("x").key(key(KeyCode::Char(' ')), &look),
            Step::Close(Outcome::No),
            "a stray Space is no"
        );
    }

    #[test]
    fn tab_moves_to_no_and_round() {
        let look = Look::default();
        let mut c = Confirm::new("Close pane?");
        assert_eq!(c.key(key(KeyCode::Tab), &look), Step::Redraw);
        let s = drawn(&mut c, &look);
        assert!(s.iter().any(|l| l.contains("  Yes  › No")), "{s:#?}");
        assert_eq!(c.key(key(KeyCode::Enter), &look), Step::Close(Outcome::No));
        let mut c = Confirm::new("x");
        c.key(key(KeyCode::Tab), &look);
        c.key(key(KeyCode::Tab), &look);
        assert_eq!(
            c.key(key(KeyCode::Char(' ')), &look),
            Step::Close(Outcome::Yes),
            "back on Yes after Tab, Space presses it"
        );
        let easy = Look {
            close: true,
            ..look
        };
        let mut c = Confirm::new("x");
        c.key(key(KeyCode::BackTab), &easy);
        assert_eq!(
            c.key(key(KeyCode::Enter), &easy),
            Step::Close(Outcome::No),
            "the close button is pressed as no"
        );
    }

    #[test]
    fn it_shows_the_question_and_takes_clicks() {
        let look = Look::default();
        let mut c = Confirm::new("Close pane?").list(vec!["sleep 100".into()]);
        let s = drawn(&mut c, &look);
        assert!(s.iter().any(|l| l.contains("Are you sure?")));
        assert!(s.iter().any(|l| l.contains("Close pane? [Y/n]")));
        assert!(s.iter().any(|l| l.contains("sleep 100")));
        let d = c.drawn.clone().unwrap();
        let (y, _) = *d.rows.last().unwrap();
        let click = |col: u16, row: u16| Mouse {
            kind: MouseKind::Press,
            button: MouseButton::Left,
            col,
            row,
            mods: Mods::NONE,
        };
        let no = d.inner.x + c.spans[1].0 + 2;
        assert_eq!(c.mouse(click(no, y), &look), Step::Close(Outcome::No));
        let yes = d.inner.x + c.spans[0].0 + 2;
        assert_eq!(c.mouse(click(yes, y), &look), Step::Close(Outcome::Yes));
        assert_eq!(c.mouse(click(0, 0), &look), Step::Close(Outcome::No));
    }
}
