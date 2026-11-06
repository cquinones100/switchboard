//! A look at a tab without going there (Space in the sidebar, a right click
//! on a tab), in a box over most of the screen, as render.js's `peekLines`
//! drew it in a popup of its own: the tab's last turn in full (your prompt,
//! all of Claude's reply, and the tool call it waits on), or its screen.
//! j and k scroll it, s switches between the two, Enter goes to the tab and
//! Esc or q closes it. Mission control's Space opens the same box.

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use super::claude::Turn;
use super::text;
use crate::mux::input::{Key, KeyCode, Mouse};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit, Place};
use crate::ui::{Look, Outcome, Overlay, Step};

/// What a peek shows: the last turn, or the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Turn,
    Screen,
}

pub struct Peek {
    /// `<number> <name>` of the tab.
    title: String,
    turn: Option<Turn>,
    /// Whether its Claude is working, for a turn with no reply yet.
    working: bool,
    screen: String,
    pub mode: Mode,
    scroll: Option<usize>,
    room: usize,
    most: usize,
    drawn: Option<Drawn>,
}

impl Peek {
    /// A peek at a tab: its last turn when it has a Claude, else its screen.
    pub fn new(title: String, turn: Option<Turn>, working: bool, screen: String) -> Peek {
        let mode = if turn.is_some() {
            Mode::Turn
        } else {
            Mode::Screen
        };
        Peek {
            title,
            turn,
            working,
            screen,
            mode,
            scroll: None,
            room: 10,
            most: 0,
            drawn: None,
        }
    }

    /// Its body, wrapped to `cols`.
    fn body(&self, cols: usize, look: &Look) -> Vec<BoxLine> {
        if self.mode == Mode::Screen {
            return self
                .screen
                .split('\n')
                .map(|l| BoxLine::text(text::cut(l, cols), look.text()))
                .collect();
        }
        let Some(turn) = &self.turn else {
            return vec![BoxLine::text(
                "No Claude turn here; s shows its screen.",
                look.dim(),
            )];
        };
        let mut body: Vec<BoxLine> = text::wrap(&text::plain(&turn.prompt), cols.saturating_sub(2))
            .into_iter()
            .enumerate()
            .map(|(i, l)| {
                BoxLine::text(
                    format!("{}{l}", if i > 0 { "  " } else { "› " }),
                    look.dim(),
                )
            })
            .collect();
        body.push(BoxLine::blank());
        let reply = turn.reply.clone().unwrap_or_else(|| {
            if self.working {
                "(working)".into()
            } else {
                "(no reply yet)".into()
            }
        });
        body.extend(
            text::layout(&text::plain(&reply), cols)
                .into_iter()
                .map(|l| BoxLine::text(l, look.text())),
        );
        if let Some(pending) = &turn.pending {
            body.push(BoxLine::blank());
            body.extend(
                text::wrap(&format!("Asking: {pending}"), cols)
                    .into_iter()
                    .map(|l| {
                        BoxLine::text(
                            l,
                            Style::new()
                                .fg(Color::Indexed(220))
                                .add_modifier(Modifier::BOLD),
                        )
                    }),
            );
        }
        body
    }

    fn scroll_to(&mut self, to: usize) -> Step {
        let to = to.min(self.most);
        if Some(to) == self.scroll {
            Step::Same
        } else {
            self.scroll = Some(to);
            Step::Redraw
        }
    }

    fn at(&self) -> usize {
        self.scroll.unwrap_or(self.most)
    }
}

impl Overlay for Peek {
    fn kind(&self) -> &'static str {
        "peek"
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        // The box's inside, less a space each side.
        let inner = (buf.area.width as usize * 90 / 100)
            .max(40)
            .min(buf.area.width as usize);
        let cols = inner.saturating_sub(4).max(10);
        let body = self.body(cols, look);
        let rows = (buf.area.height as usize * 85 / 100)
            .max(10)
            .min(buf.area.height as usize);
        self.room = rows.saturating_sub(4).max(1);
        self.most = body.len().saturating_sub(self.room);
        // A turn opens at its end, where the reply finishes.
        let at = self.at().min(self.most);
        let lines: Vec<BoxLine> = body
            .into_iter()
            .skip(at)
            .take(self.room)
            .map(|mut l| {
                l.spans.insert(0, Span::raw(" "));
                l
            })
            .collect();
        let title = format!(
            "Peek {} · {}",
            self.title,
            if self.mode == Mode::Screen {
                "screen"
            } else {
                "last turn"
            }
        );
        let keys = format!(
            "jk scroll  s {}  ↵ go to tab  esc back",
            if self.mode == Mode::Screen {
                "last turn"
            } else {
                "screen"
            }
        );
        let mut spec = BoxSpec::new(&title, &lines);
        spec.keys = Some(&keys);
        spec.place = Place::Share {
            cols_pct: 90,
            rows_pct: 85,
        };
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, _look: &Look) -> Step {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.scroll_to(self.at() + 1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_to(self.at().saturating_sub(1)),
            KeyCode::Char(' ') | KeyCode::PageDown => self.scroll_to(self.at() + self.room),
            KeyCode::PageUp => self.scroll_to(self.at().saturating_sub(self.room)),
            KeyCode::Char('g') | KeyCode::Home => self.scroll_to(0),
            KeyCode::Char('G') | KeyCode::End => self.scroll_to(self.most),
            KeyCode::Char('s') => {
                self.mode = match self.mode {
                    Mode::Turn => Mode::Screen,
                    Mode::Screen => Mode::Turn,
                };
                self.scroll = None;
                Step::Redraw
            }
            KeyCode::Enter => Step::Close(Outcome::Yes),
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('h') => {
                Step::Close(Outcome::Dismissed)
            }
            _ => Step::Same,
        }
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        match draw::gesture(&mouse) {
            Gesture::WheelDown => self.scroll_to(self.at() + 1),
            Gesture::WheelUp => self.scroll_to(self.at().saturating_sub(1)),
            Gesture::Click if look.mouse => {
                match self.drawn.as_ref().map(|d| d.hit(mouse.col, mouse.row)) {
                    Some(Hit::Outside | Hit::Close) => Step::Close(Outcome::Dismissed),
                    _ => Step::Same,
                }
            }
            _ => Step::Same,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::input::Mods;
    use crate::ui::draw::tests::screen;
    use ratatui::layout::Rect;

    fn key(c: char) -> Key {
        Key::new(KeyCode::Char(c), Mods::NONE)
    }

    #[test]
    fn it_shows_the_last_turn_or_the_screen_and_enter_goes_there() {
        let turn = Turn {
            prompt: "fix the flaky test".into(),
            reply: Some("Fixed it.".into()),
            pending: Some("Bash: git push".into()),
            ..Turn::default()
        };
        let mut peek = Peek::new("3 api".into(), Some(turn), false, "$ ls\nsrc".into());
        let look = Look::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        peek.draw(&mut buf, &look);
        let text = screen(&buf).join("\n");
        assert!(text.contains("Peek 3 api · last turn"), "{text}");
        assert!(text.contains("› fix the flaky test"));
        assert!(text.contains("Fixed it."));
        assert!(text.contains("Asking: Bash: git push"));
        assert_eq!(peek.key(key('s'), &look), Step::Redraw);
        peek.draw(&mut buf, &look);
        let text = screen(&buf).join("\n");
        assert!(text.contains("· screen") && text.contains("$ ls"), "{text}");
        assert_eq!(
            peek.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
            Step::Close(Outcome::Yes)
        );
        assert_eq!(
            peek.key(Key::new(KeyCode::Esc, Mods::NONE), &look),
            Step::Close(Outcome::Dismissed)
        );
        let shell = Peek::new("1 sh".into(), None, false, "$".into());
        assert_eq!(shell.mode, Mode::Screen);
    }
}
