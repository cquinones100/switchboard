//! Ctrl-F: find a note, ported from notes/lua/notes/finder.lua, which ran
//! fzf in a floating window. Every note but those in Recently Deleted is
//! listed as the list lists them (pinned first, then newest), each as its
//! title, its folder and how it starts, any of which you can type to match,
//! with the note itself shown beside the list. Enter opens it; Escape closes
//! the finder.

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::store::NoteEntry;
use crate::mux::input::{Key, KeyCode, Mouse, MouseButton, MouseKind};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit, Place};
use crate::ui::{Look, Outcome, Overlay, Picked, Step};

pub struct Finder {
    notes: Vec<NoteEntry>,
    query: String,
    shown: Vec<usize>,
    cursor: usize,
    top: usize,
    room: usize,
    drawn: Option<Drawn>,
    matcher: Matcher,
    /// The column of the line between the list and the preview, which is
    /// the list's scroll bar while it runs past the box.
    bar_x: Option<u16>,
    /// The scroll bar's thumb being dragged, from where (`kit::scroll_grab`).
    dragging: Option<crate::ui::kit::Grab>,
}

/// What a note is matched by: its title, its folder and how it starts.
fn haystack(n: &NoteEntry) -> String {
    let folder = if n.folder.is_empty() {
        "Notes"
    } else {
        &n.folder
    };
    format!("{} {folder} {}", n.title, n.preview)
}

impl Finder {
    pub fn new(notes: Vec<NoteEntry>) -> Finder {
        let mut f = Finder {
            shown: (0..notes.len()).collect(),
            notes,
            query: String::new(),
            cursor: 0,
            top: 0,
            room: 10,
            drawn: None,
            matcher: Matcher::new(Config::DEFAULT),
            bar_x: None,
            dragging: None,
        };
        f.refilter();
        f
    }

    /// The rows shown, by the notes' titles.
    pub fn shown(&self) -> Vec<&str> {
        self.shown
            .iter()
            .map(|&i| self.notes[i].title.as_str())
            .collect()
    }

    fn refilter(&mut self) {
        let pattern = Pattern::new(
            &self.query,
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        let mut buf = Vec::new();
        // Kept in the list's order, as fzf's --tiebreak=index kept them.
        self.shown = (0..self.notes.len())
            .filter(|&i| {
                let hay = haystack(&self.notes[i]);
                self.query.is_empty()
                    || pattern
                        .score(Utf32Str::new(&hay, &mut buf), &mut self.matcher)
                        .is_some()
            })
            .collect();
        self.cursor = 0;
        self.top = 0;
    }

    fn pick(&self) -> Step {
        match self.shown.get(self.cursor) {
            Some(&i) => Step::Close(Outcome::Picked(Picked {
                index: i,
                value: self.notes[i].path.to_string_lossy().into_owned(),
                query: self.query.clone(),
            })),
            None => Step::Close(Outcome::Dismissed),
        }
    }

    fn move_to(&mut self, to: usize) -> Step {
        self.cursor = to.min(self.shown.len().saturating_sub(1));
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + self.room {
            self.top = self.cursor + 1 - self.room;
        }
        Step::Redraw
    }
}

impl Overlay for Finder {
    fn kind(&self) -> &'static str {
        "finder"
    }

    fn stands_out(&self) -> Option<ratatui::layout::Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let mut lines = vec![BoxLine::spans(vec![
            Span::styled(" Find a note: ", Style::new().fg(draw::EDGE)),
            Span::styled(self.query.clone(), look.bright()),
            Span::styled("▏", look.dim()),
        ])];
        lines.push(BoxLine::text(
            format!("   {}/{}", self.shown.len(), self.notes.len()),
            look.dim(),
        ));
        let mut spec = BoxSpec::new("Find a note", &[]);
        spec.place = Place::Share {
            cols_pct: 85,
            rows_pct: 80,
        };
        spec.keys = Some("type to filter  ↑↓ move  ↵ open  esc close");
        spec.closes = false;
        let area = buf.area;
        let inner_w = (area.width as usize * 85 / 100)
            .max(40)
            .min(area.width as usize)
            - 2;
        let inner_h = (area.height as usize * 80 / 100)
            .max(10)
            .min(area.height as usize);
        self.room = inner_h.saturating_sub(2 + 2 + 2).max(1);
        if self.cursor >= self.top + self.room {
            self.top = self.cursor + 1 - self.room;
        }
        let list_w = (inner_w * 55 / 100).max(20);
        for row in self.top..(self.top + self.room).min(self.shown.len()) {
            let n = &self.notes[self.shown[row]];
            let chosen = row == self.cursor;
            let folder = if n.folder.is_empty() {
                "Notes"
            } else {
                &n.folder
            };
            let mark = if n.pinned { "● " } else { "  " };
            let text = format!(
                "{} {}  {}",
                draw::fit(&format!("{mark}{}", n.title), 32),
                draw::fit(folder, 14),
                n.preview
            );
            let spans = vec![
                if chosen {
                    look.pointer()
                } else {
                    Span::raw(" ")
                },
                Span::styled(
                    format!(" {}", draw::fit(&text, list_w.saturating_sub(3))),
                    if chosen {
                        look.bold_bright()
                    } else {
                        look.text()
                    },
                ),
            ];
            lines.push(
                BoxLine::spans(spans)
                    .fill(chosen.then_some(look.theme.highlight))
                    .clickable(),
            );
        }
        spec.lines = &lines;
        let drawn = draw::draw_box(buf, &spec, look);
        // The note under the cursor, beside the list.
        let preview = Rect::new(
            drawn.inner.x + list_w as u16 + 1,
            drawn.inner.y + 2,
            drawn.inner.width.saturating_sub(list_w as u16 + 2),
            drawn.inner.height.saturating_sub(4),
        );
        self.bar_x = None;
        if preview.width > 10 {
            for y in preview.y..preview.bottom() {
                buf[(preview.x - 1, y)]
                    .set_symbol("│")
                    .set_style(look.faint());
            }
            // The line beside the list is its scroll bar while it runs past
            // the box: the thumb a heavier line beside the rows it stands for.
            if look.scroll_bars
                && let Some((at, len)) =
                    crate::ui::kit::thumb(self.shown.len(), self.top, self.room, self.room)
            {
                for (y, i) in &drawn.rows {
                    if (2 + at..2 + at + len).contains(i) {
                        buf[(preview.x - 1, *y)].set_symbol("┃");
                    }
                }
                self.bar_x = Some(preview.x - 1);
            }
            if let Some(&i) = self.shown.get(self.cursor) {
                let text = &self.notes[i].text;
                let mut y = preview.y;
                'lines: for line in text.lines() {
                    for piece in draw::wrap(line, preview.width as usize) {
                        if y >= preview.bottom() {
                            break 'lines;
                        }
                        draw::put_line(
                            buf,
                            preview.x,
                            y,
                            preview.width,
                            &Line::styled(piece, look.text()),
                        );
                        y += 1;
                    }
                }
            }
        }
        self.drawn = Some(drawn);
    }

    fn key(&mut self, key: Key, _look: &Look) -> Step {
        let ctrl = key.mods.ctrl;
        match key.code {
            KeyCode::Enter => self.pick(),
            KeyCode::Esc => Step::Close(Outcome::Dismissed),
            KeyCode::Char('c' | 'g') if ctrl => Step::Close(Outcome::Dismissed),
            KeyCode::Down | KeyCode::Tab => self.move_to(self.cursor + 1),
            KeyCode::Char('n' | 'j') if ctrl => self.move_to(self.cursor + 1),
            KeyCode::Up | KeyCode::BackTab => self.move_to(self.cursor.saturating_sub(1)),
            KeyCode::Char('p' | 'k') if ctrl => self.move_to(self.cursor.saturating_sub(1)),
            KeyCode::Backspace => {
                self.query.pop();
                self.refilter();
                Step::Redraw
            }
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.refilter();
                Step::Redraw
            }
            KeyCode::Char(c) if !ctrl && !key.mods.alt => {
                self.query.push(c);
                self.refilter();
                Step::Redraw
            }
            _ => Step::Same,
        }
    }

    fn paste(&mut self, text: &str, _look: &Look) -> Step {
        self.query.push_str(text.lines().next().unwrap_or(""));
        self.refilter();
        Step::Redraw
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        // The list's scroll bar: a press pages toward it, a drag moves the
        // list, the cursor kept on a row shown.
        // A drag under way follows the mouse up and down whatever its column.
        let bar = match self.dragging {
            Some(_) => draw::edge_bar_drag_row(drawn, 2, mouse.row),
            None => self
                .bar_x
                .filter(|x| *x == mouse.col && look.mouse)
                .and_then(|_| drawn.rows.iter().find(|(y, _)| *y == mouse.row))
                .and_then(|(_, i)| i.checked_sub(2))
                .filter(|r| *r < self.room),
        };
        let (total, room) = (self.shown.len(), self.room);
        let to = match (mouse.kind, mouse.button, bar) {
            (MouseKind::Press, MouseButton::Left, Some(row)) => {
                self.dragging = Some(crate::ui::kit::scroll_grab(
                    total, self.top, room, room, row,
                ));
                Some(
                    crate::ui::kit::scroll_press(total, self.top, room, room, row)
                        .unwrap_or(self.top),
                )
            }
            (MouseKind::Drag, _, Some(row)) if self.dragging.is_some() => {
                let grab = self.dragging.unwrap_or_default();
                Some(crate::ui::kit::scroll_drag(total, room, room, row, grab))
            }
            (MouseKind::Drag, ..) if self.dragging.is_some() => return Step::Same,
            (MouseKind::Release, ..) if self.dragging.is_some() => {
                self.dragging = None;
                return Step::Same;
            }
            _ => None,
        };
        if let Some(to) = to {
            self.top = to;
            self.cursor = self.cursor.clamp(to, to + room - 1).min(total - 1);
            return Step::Redraw;
        }
        let hit = drawn.hit(mouse.col, mouse.row);
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse => match hit {
                Hit::Outside => Step::Close(Outcome::Dismissed),
                Hit::Line(i, _) if i >= 2 => {
                    let row = self.top + i - 2;
                    if row == self.cursor {
                        return self.pick();
                    }
                    self.move_to(row)
                }
                _ => Step::Same,
            },
            Gesture::WheelDown => self.move_to(self.cursor + 1),
            Gesture::WheelUp => self.move_to(self.cursor.saturating_sub(1)),
            _ => Step::Same,
        }
    }

    fn cursor(&self) -> Option<(u16, u16)> {
        let d = self.drawn.as_ref()?;
        Some((d.inner.x + 14 + draw::width(&self.query) as u16, d.inner.y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::input::Mods;
    use crate::notes::store::tests::{set_mtime, temp};

    #[test]
    fn the_finder_lists_every_note_newest_first_and_matches_what_is_typed() {
        let (_dir, mut s) = temp("finder");
        let standup = s.create("Work", Some("Standup"));
        set_mtime(&standup, crate::util::now_secs() - 60);
        s.create("", Some("Groceries"));
        let gone = s.create("", Some("Gone"));
        s.delete(&gone);
        let mut f = Finder::new(s.notes(None));
        assert_eq!(
            f.shown(),
            ["Groceries", "Standup"],
            "Recently Deleted isn't listed"
        );
        for c in "work".chars() {
            f.key(Key::new(KeyCode::Char(c), Mods::NONE), &Look::default());
        }
        assert_eq!(f.shown(), ["Standup"], "its folder matches too");
        match f.key(Key::new(KeyCode::Enter, Mods::NONE), &Look::default()) {
            Step::Close(Outcome::Picked(p)) => assert!(p.value.ends_with("Work/Standup.md")),
            _ => panic!("Enter opens the one picked"),
        }
    }
}
