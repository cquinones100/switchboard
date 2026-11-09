//! A box of lines to read over the workspace: what a popup running a
//! script showed until a key (habits-report.sh, repo-update.sh's
//! `run-setup-check` and `run-setup-part`). It is as the list of keys is: j
//! and k (the arrows, the wheel, Space and the page keys) scroll it, any
//! other key or a click outside closes it. A line too long for the box is
//! wrapped, in its first piece's tone.

use ratatui::buffer::Buffer;
use ratatui::style::Style;
use ratatui::text::Span;

use crate::mux::input::{Key, KeyCode, Mouse};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use crate::ui::{Look, Outcome, Overlay, Step};
pub use crate::usage::report::{Line, Tone};

/// The widest the inside gets, as the popups' `-w 90`.
const WIDEST: usize = 96;

pub struct TextBox {
    title: String,
    lines: Vec<Line>,
    scroll: usize,
    /// How many lines the last draw showed, and how many there were once
    /// wrapped.
    room: usize,
    wrapped: usize,
    drawn: Option<Drawn>,
}

impl TextBox {
    pub fn new(title: impl Into<String>, lines: Vec<Line>) -> TextBox {
        TextBox {
            title: title.into(),
            lines,
            scroll: 0,
            room: 10,
            wrapped: 0,
            drawn: None,
        }
    }

    /// Plain text, a line each.
    pub fn plain(title: impl Into<String>, text: &str) -> TextBox {
        let lines = text
            .lines()
            .map(|l| vec![(format!(" {l}"), Tone::Plain)])
            .collect();
        TextBox::new(title, lines)
    }

    /// The lines, as shown, for the tests.
    pub fn text(&self) -> Vec<String> {
        self.lines.iter().map(crate::usage::report::plain).collect()
    }

    fn scroll_to(&mut self, to: usize) -> Step {
        let to = to.min(self.wrapped.saturating_sub(self.room));
        if to == self.scroll {
            Step::Same
        } else {
            self.scroll = to;
            Step::Redraw
        }
    }
}

fn style(tone: Tone, look: &Look) -> Style {
    match tone {
        Tone::Plain => look.text(),
        Tone::Heading => look.bold_bright(),
        Tone::Dim => look.dim(),
    }
}

impl Overlay for TextBox {
    fn kind(&self) -> &'static str {
        "text"
    }

    fn stands_out(&self) -> Option<ratatui::layout::Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let inner = (buf.area.width as usize).saturating_sub(4).min(WIDEST);
        let mut all: Vec<BoxLine> = Vec::new();
        for line in &self.lines {
            let width: usize = line.iter().map(|(t, _)| draw::width(t)).sum();
            if width <= inner {
                all.push(BoxLine::spans(
                    line.iter()
                        .map(|(t, tone)| Span::styled(t.clone(), style(*tone, look)))
                        .collect(),
                ));
                continue;
            }
            let tone = line.first().map_or(Tone::Plain, |(_, t)| *t);
            let text = crate::usage::report::plain(line);
            let indent = text.len() - text.trim_start().len();
            let pad = " ".repeat(indent);
            for part in draw::wrap(text.trim_start(), inner.saturating_sub(indent + 1)) {
                all.push(BoxLine::text(format!("{pad}{part}"), style(tone, look)));
            }
        }
        // A row clear above and below, the keys' two rows and the edges.
        self.room = (buf.area.height as usize).saturating_sub(8).max(1);
        self.wrapped = all.len();
        self.scroll = self.scroll.min(self.wrapped.saturating_sub(self.room));
        let shown: Vec<BoxLine> = all.into_iter().skip(self.scroll).take(self.room).collect();
        let mut spec = BoxSpec::new(&self.title, &shown);
        spec.keys = Some(if self.wrapped > self.room {
            "jk scroll  any other key closes"
        } else {
            "any key closes"
        });
        spec.width = inner as u16;
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, _look: &Look) -> Step {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.scroll_to(self.scroll + 1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_to(self.scroll.saturating_sub(1)),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_to(self.scroll + self.room),
            KeyCode::PageUp => self.scroll_to(self.scroll.saturating_sub(self.room)),
            _ => Step::Close(Outcome::Dismissed),
        }
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        match draw::gesture(&mouse) {
            Gesture::WheelDown => self.scroll_to(self.scroll + 1),
            Gesture::WheelUp => self.scroll_to(self.scroll.saturating_sub(1)),
            Gesture::Click if look.mouse => {
                let hit = self.drawn.as_ref().map(|d| d.hit(mouse.col, mouse.row));
                match hit {
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

    #[test]
    fn long_lines_wrap_and_it_scrolls_until_another_key() {
        let mut text: String = (0..30).map(|i| format!("line {i}\n")).collect();
        text.push_str(&"word ".repeat(40));
        let mut b = TextBox::plain("Setup", &text);
        let look = Look::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 20));
        b.draw(&mut buf, &look);
        let shown = screen(&buf);
        assert!(shown.iter().any(|l| l.contains("line 0")));
        assert!(shown.iter().any(|l| l.contains("jk scroll")));
        for _ in 0..40 {
            b.key(Key::new(KeyCode::Char('j'), Mods::NONE), &look);
        }
        b.draw(&mut buf, &look);
        let shown = screen(&buf);
        assert!(!shown.iter().any(|l| l.contains("line 0 ")));
        assert!(
            shown.iter().filter(|l| l.contains("word word")).count() > 1,
            "the long line wrapped:\n{}",
            shown.join("\n")
        );
        assert_eq!(
            b.key(Key::new(KeyCode::Char('x'), Mods::NONE), &look),
            Step::Close(Outcome::Dismissed)
        );
    }
}
