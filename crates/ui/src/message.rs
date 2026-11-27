//! A message in a small box in the middle of the screen (tmux/say.sh), in
//! place of tmux's message on the status line, which is easy to miss and
//! gone in a moment: as wide as its text, which the box wraps to 72
//! columns as every box's (`draw::WRAP`), and as tall as it; any key closes it, and the key goes no further; a click closes it
//! too; left alone, it closes itself after a few seconds. One may carry a
//! list under its text (`Message::with_list`: the commits a restart brought
//! in, a short hash dim and a subject each), which stays up until a key
//! closes it, and scrolls with `j` and `k` (and the wheel) when it is
//! taller than the screen.

use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::text::Span;

use crate::Look;
use crate::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use crate::input::{Key, Mouse};
use crate::overlay::{Outcome, Overlay, Step};

/// How long a message stays up (say.sh's `-t`).
pub const SECONDS: u64 = 5;
/// The widest a row of its list runs.
const WRAP: usize = draw::WRAP;

/// One row of a message's list: a short hash, drawn dim, and its words;
/// a row with no hash (`and 3 more`) is all dim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListRow {
    pub hash: String,
    pub words: String,
}

pub struct Message {
    text: String,
    list: Vec<ListRow>,
    /// The first of the box's rows shown, while it is taller than the screen.
    top: usize,
    /// How many of its rows the screen had room for, and how many it has
    /// as the box wraps them, as last drawn.
    room: usize,
    count: usize,
    /// None while it has a list, which waits for a key.
    until: Option<Instant>,
    close_lit: bool,
    drawn: Option<Drawn>,
}

impl Message {
    pub fn new(text: &str, seconds: u64) -> Message {
        Message {
            text: text.trim_end().to_string(),
            list: Vec::new(),
            top: 0,
            room: usize::MAX,
            count: 0,
            until: Some(Instant::now() + Duration::from_secs(seconds)),
            close_lit: false,
            drawn: None,
        }
    }

    /// A message with `list` under its text, up until a key closes it.
    pub fn with_list(text: &str, list: Vec<ListRow>) -> Message {
        let mut m = Message::new(text, SECONDS);
        if !list.is_empty() {
            m.until = None;
        }
        m.list = list;
        m
    }

    /// The list under the text.
    pub fn list(&self) -> &[ListRow] {
        &self.list
    }

    /// Every row inside the box: a blank one, the text, and the list after
    /// a blank one.
    fn rows(&self, look: &Look) -> Vec<BoxLine> {
        let mut rows: Vec<BoxLine> = std::iter::once(BoxLine::blank())
            .chain(
                self.text
                    .split('\n')
                    .map(|l| BoxLine::text(format!("  {l}"), look.text()).wraps()),
            )
            .collect();
        if !self.list.is_empty() {
            rows.push(BoxLine::blank());
        }
        for row in &self.list {
            let line = if row.hash.is_empty() {
                BoxLine::text(format!("  {}", draw::cut(&row.words, WRAP)), look.dim())
            } else {
                let room = WRAP.saturating_sub(draw::width(&row.hash) + 1);
                BoxLine::spans(vec![
                    Span::styled(format!("  {}", row.hash), look.dim()),
                    Span::styled(format!(" {}", draw::cut(&row.words, room)), look.text()),
                ])
            };
            rows.push(line);
        }
        rows
    }

    /// Moves the rows shown by `by`, while there are more than fit.
    fn scroll(&mut self, by: isize) -> Step {
        let last = self.count.saturating_sub(self.room);
        let to = self.top.saturating_add_signed(by).min(last);
        if to == self.top {
            Step::Same
        } else {
            self.top = to;
            Step::Redraw
        }
    }

    /// Whether the box is taller than the screen, so `j` and `k` scroll it.
    fn scrolls(&self) -> bool {
        self.count > self.room
    }

    /// The text, as given.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Its box: no title, at least 30 columns wide.
    fn spec<'a>(&self, rows: &'a [BoxLine]) -> BoxSpec<'a> {
        let mut spec = BoxSpec::new("", rows);
        spec.width = 28;
        spec.close_lit = self.close_lit;
        spec
    }
}

impl Overlay for Message {
    fn drawn(&self) -> Option<&crate::draw::Drawn> {
        self.drawn.as_ref()
    }

    fn kind(&self) -> &'static str {
        "message"
    }

    fn stands_out(&self) -> Option<ratatui::layout::Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        // A blank row above the text, which is indented two columns, as
        // say.sh printed it; at least 30 columns wide.
        let rows = self.rows(look);
        let rows = draw::wrapped(&self.spec(&rows), buf.area.width, look);
        self.count = rows.len();
        // A row clear above and below, as draw_box keeps, and the keys'
        // two while it scrolls.
        let fits = (buf.area.height as usize).saturating_sub(4).max(1);
        self.room = if rows.len() > fits {
            fits.saturating_sub(2).max(1)
        } else {
            fits
        };
        self.top = self.top.min(rows.len().saturating_sub(self.room));
        let shown = &rows[self.top..];
        let mut spec = self.spec(shown);
        if self.scrolls() {
            spec.keys = Some("j k scroll   any other key closes");
        }
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, _look: &Look) -> Step {
        match crate::kit::nav::step(key).and_then(|m| m.rows()) {
            Some(by) if self.scrolls() => self.scroll(by),
            _ => Step::Close(Outcome::Dismissed),
        }
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        match draw::gesture(&mouse) {
            Gesture::Click => Step::Close(Outcome::Dismissed),
            Gesture::WheelDown if self.scrolls() => self.scroll(1),
            Gesture::WheelUp if self.scrolls() => self.scroll(-1),
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
        self.until
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::tests::screen;
    use crate::input::{KeyCode, Mods, MouseButton, MouseKind};
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
        let mut buf = Buffer::empty(Rect::new(0, 0, 120, 24));
        m.draw(&mut buf, &Look::default());
        let d = m.drawn.clone().unwrap();
        assert_eq!(d.rows.len(), 4, "a blank row and three of text");
        // Its longest row, 14 words of 72 columns at most, its indent and
        // margins.
        assert_eq!(d.outer.width, 75);
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

    #[test]
    fn a_list_stays_up_and_scrolls_when_it_doesnt_fit() {
        let list: Vec<ListRow> = (1..=15)
            .map(|n| ListRow {
                hash: format!("abc{n:04}"),
                words: format!("Commit {n}"),
            })
            .collect();
        let mut m = Message::with_list("Restarted.", list);
        assert_eq!(m.deadline(), None, "waits for a key");
        let look = Look::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 40));
        m.draw(&mut buf, &look);
        let s = screen(&buf).join("\n");
        assert!(
            s.contains("abc0001 Commit 1") && s.contains("abc0015 Commit 15"),
            "{s}"
        );
        assert!(!s.contains("j k scroll"), "it fits: {s}");
        let j = Key::new(KeyCode::Char('j'), Mods::NONE);
        assert_eq!(m.key(j, &look), Step::Close(Outcome::Dismissed));

        let mut m = Message::with_list(
            "Restarted.",
            vec![ListRow {
                hash: String::new(),
                words: "and 2 more".into(),
            }]
            .into_iter()
            .chain((1..=15).map(|n| ListRow {
                hash: format!("abc{n:04}"),
                words: format!("Commit {n}"),
            }))
            .collect(),
        );
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 12));
        m.draw(&mut buf, &look);
        let s = screen(&buf).join("\n");
        assert!(s.contains("j k scroll") && !s.contains("Commit 15"), "{s}");
        for _ in 0..30 {
            if m.key(j, &look) != Step::Redraw {
                break;
            }
        }
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 12));
        m.draw(&mut buf, &look);
        let s = screen(&buf).join("\n");
        assert!(
            s.contains("abc0015 Commit 15") && !s.contains("Restarted."),
            "{s}"
        );
        let x = Key::new(KeyCode::Char('x'), Mods::NONE);
        assert_eq!(m.key(x, &look), Step::Close(Outcome::Dismissed));
    }
}
