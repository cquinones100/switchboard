//! The list of keys (`M-b ?`, what tmux's `list-keys` showed): every binding
//! and what it does, in a box over the middle of the screen, as the ? keys
//! box in mission control (render.js's `helpModal`). j and k (the arrows,
//! the wheel, Space and the page keys) scroll it; any other key or a click
//! closes it.

use ratatui::buffer::Buffer;
use ratatui::text::Span;

use super::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use super::{Look, Outcome, Overlay, Step, kit};
use crate::mux::input::{Key, KeyCode, Mouse, MouseButton, MouseKind};

pub struct KeyList {
    rows: Vec<(String, String)>,
    scroll: usize,
    /// How many rows the last draw showed.
    room: usize,
    drawn: Option<Drawn>,
    /// Its scroll bar's thumb being dragged, from where (`kit::scroll_grab`).
    dragging: Option<crate::ui::kit::Grab>,
}

impl KeyList {
    /// The list of (keys, what they do).
    pub fn new(rows: Vec<(String, String)>) -> KeyList {
        KeyList {
            rows,
            scroll: 0,
            room: 10,
            drawn: None,
            dragging: None,
        }
    }

    fn scroll_to(&mut self, to: usize) -> Step {
        let to = to.min(self.rows.len().saturating_sub(self.room));
        if to == self.scroll {
            Step::Same
        } else {
            self.scroll = to;
            Step::Redraw
        }
    }
}

impl Overlay for KeyList {
    fn drawn(&self) -> Option<&crate::ui::draw::Drawn> {
        self.drawn.as_ref()
    }

    fn kind(&self) -> &'static str {
        "keys"
    }

    fn stands_out(&self) -> Option<ratatui::layout::Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        self.room = (buf.area.height as usize).saturating_sub(6).max(1);
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(self.room));
        let key_w = self
            .rows
            .iter()
            .map(|(k, _)| draw::width(k))
            .max()
            .unwrap_or(0);
        let lines: Vec<BoxLine> = self
            .rows
            .iter()
            .skip(self.scroll)
            .take(self.room)
            .map(|(keys, about)| {
                BoxLine::spans(vec![
                    Span::raw(" "),
                    Span::styled(draw::fit(keys, key_w), look.bold_bright()),
                    Span::raw("  "),
                    Span::styled(about.clone(), look.text()),
                ])
            })
            .collect();
        let mut spec = BoxSpec::new("Keys", &lines);
        spec.keys = Some(if self.rows.len() > self.room {
            "jk scroll  any other key closes"
        } else {
            "any key closes"
        });
        let drawn = draw::draw_box(buf, &spec, look);
        draw::edge_bar(
            buf,
            &drawn,
            (0, self.room),
            (self.rows.len(), self.scroll),
            look,
        );
        self.drawn = Some(drawn);
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
        // Its scroll bar: a press pages toward it, a drag moves the list.
        let bar = self
            .drawn
            .as_ref()
            .and_then(|d| match self.dragging {
                Some(_) => draw::edge_bar_drag_row(d, 0, mouse.row),
                None => draw::edge_bar_row(d, (0, self.room), mouse.col, mouse.row),
            })
            .filter(|_| look.mouse && look.scroll_bars && self.rows.len() > self.room);
        let (total, room) = (self.rows.len(), self.room);
        match (mouse.kind, mouse.button, bar) {
            (MouseKind::Press, MouseButton::Left, Some(row)) => {
                self.dragging = Some(kit::scroll_grab(total, self.scroll, room, room, row));
                let to = kit::scroll_press(total, self.scroll, room, room, row);
                return self.scroll_to(to.unwrap_or(self.scroll));
            }
            (MouseKind::Drag, _, Some(row)) if self.dragging.is_some() => {
                let grab = self.dragging.unwrap_or_default();
                return self.scroll_to(kit::scroll_drag(total, room, room, row, grab));
            }
            (MouseKind::Drag, ..) if self.dragging.is_some() => return Step::Same,
            (MouseKind::Release, ..) if self.dragging.is_some() => {
                self.dragging = None;
                return Step::Same;
            }
            _ => {}
        }
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
    fn it_scrolls_and_any_other_key_closes_it() {
        let rows: Vec<_> = (0..40)
            .map(|i| (format!("M-{i}"), format!("thing {i}")))
            .collect();
        let mut list = KeyList::new(rows);
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 20));
        list.draw(&mut buf, &Look::default());
        assert!(screen(&buf).iter().any(|l| l.contains("M-0   thing 0")));
        let look = Look::default();
        assert_eq!(
            list.key(Key::new(KeyCode::Char('j'), Mods::NONE), &look),
            Step::Redraw
        );
        list.draw(&mut buf, &look);
        assert!(!screen(&buf).iter().any(|l| l.contains("thing 0")));
        assert_eq!(
            list.key(Key::new(KeyCode::Char('x'), Mods::NONE), &look),
            Step::Close(Outcome::Dismissed)
        );
    }

    #[test]
    fn its_right_edge_is_a_scroll_bar_that_pages_and_drags_it() {
        let rows: Vec<_> = (0..40)
            .map(|i| (format!("M-{i}"), format!("thing {i}")))
            .collect();
        let mut list = KeyList::new(rows);
        let look = Look::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 20));
        list.draw(&mut buf, &look);
        let drawn = list.drawn.clone().unwrap();
        let x = drawn.outer.right() - 1;
        let edge: String = drawn
            .rows
            .iter()
            .map(|(y, _)| buf[(x, *y)].symbol().to_string())
            .collect();
        assert!(edge.starts_with('┃') && edge.ends_with('│'), "{edge}");
        let at = |kind, row| Mouse {
            kind,
            button: MouseButton::Left,
            col: x,
            row,
            mods: Mods::NONE,
        };
        let (last_y, _) = *drawn.rows.last().unwrap();
        assert_eq!(
            list.mouse(at(MouseKind::Press, last_y), &look),
            Step::Redraw
        );
        assert_eq!(list.scroll, list.room, "a page down, the box still open");
        let (first_y, _) = drawn.rows[0];
        list.mouse(at(MouseKind::Drag, first_y), &look);
        assert_eq!(list.scroll, 0, "dragged to the top");
        list.mouse(at(MouseKind::Release, first_y), &look);
        assert!(list.dragging.is_none());
    }
}
