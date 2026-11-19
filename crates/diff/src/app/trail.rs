//! Back and forward, as a browser's: where the reader was before each jump
//! (a view switched, a page read from the outline or the map, `]` and `[`,
//! a change shown in the files, a click that lands elsewhere), kept so `H`
//! and the bar's ← go back to it and `L` and → forward again. Stepping
//! (`j` and `k`, scrolling, the wheel, Tab between panes) is no jump, so
//! moving down a list or a column leaves nothing to go back through.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::map::Mapper;
use super::reading::{Mode, Place_};
use super::{App, Focus};

/// How many places back are kept.
const KEPT: usize = 100;

/// A place to go back to: the view, and where each view was in it.
#[derive(Clone, Debug)]
pub(super) struct Spot {
    mode: Mode,
    files: Place_,
    reader: (usize, usize, usize),
    mapper: Mapper,
    focus: Focus,
}

/// Where the reader is, to tell a jump: the view, its file, page or box,
/// and in the Story view whether the column is being read.
type Where = (Mode, usize, Option<usize>, usize, bool);

impl App {
    fn where_now(&mut self) -> Where {
        match self.mode {
            Mode::Files => (Mode::Files, self.at, self.alone, 0, false),
            Mode::Story => {
                let reading = self.focus != Focus::List;
                (Mode::Story, self.section(), None, 0, reading)
            }
            Mode::Map => (Mode::Map, self.mapper.at, None, self.mapper.chapter, false),
        }
    }

    fn trail_spot(&mut self) -> Spot {
        Spot {
            mode: self.mode,
            files: self.place(),
            reader: (self.reader.item, self.reader.cur, self.reader.top),
            mapper: self.mapper.clone(),
            focus: self.focus,
        }
    }

    /// Whether a key only steps: moving down a list or a column, or
    /// scrolling, which is no jump.
    fn steps(key: &KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        matches!(
            key.code,
            KeyCode::Char('j' | 'k' | 'J' | 'K' | 'h' | 'l' | 'G')
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::Tab
                | KeyCode::BackTab
        ) || (ctrl && matches!(key.code, KeyCode::Char('d' | 'u' | 'e' | 'y' | 'f' | 'b')))
    }

    /// Where the reader is before a key that may jump.
    pub(super) fn before_key(&mut self, key: &KeyEvent) -> Option<(Where, Spot)> {
        (!Self::steps(key) && self.change.is_some()).then(|| (self.where_now(), self.trail_spot()))
    }

    /// Where the reader is before a click, which may jump.
    pub(super) fn before_click(&mut self, m: &MouseEvent) -> Option<(Where, Spot)> {
        let click = matches!(m.kind, MouseEventKind::Down(MouseButton::Left));
        (click && self.change.is_some()).then(|| (self.where_now(), self.trail_spot()))
    }

    /// After a key or a click: a jump keeps where it left, and nothing
    /// forward any more.
    pub(super) fn after(&mut self, before: Option<(Where, Spot)>) {
        let Some((was, spot)) = before else {
            return;
        };
        if was != self.where_now() {
            self.back.push(spot);
            if self.back.len() > KEPT {
                self.back.remove(0);
            }
            self.ahead.clear();
        }
    }

    /// `H` and `L` on the diff itself, not typed into a box or a filter.
    pub(super) fn trail_key(&self, key: &KeyEvent) -> Option<bool> {
        let plain = key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT;
        let free = self.menu.is_none()
            && self.settings.is_none()
            && self.ask_box.is_none()
            && self.answer.is_none()
            && self.history.is_none()
            && !self.filtering
            && self.pending.is_none();
        match key.code {
            KeyCode::Char('H') if plain && free => Some(false),
            KeyCode::Char('L') if plain && free => Some(true),
            _ => None,
        }
    }

    /// Back where the reader was before the last jump (`forward` false),
    /// or forward again.
    pub(super) fn go_along(&mut self, forward: bool) {
        let to = if forward {
            self.ahead.pop()
        } else {
            self.back.pop()
        };
        let Some(to) = to else {
            return self.say(if forward {
                "Nothing to go forward to."
            } else {
                "Nothing to go back to."
            });
        };
        let here = self.trail_spot();
        if forward {
            self.back.push(here);
        } else {
            self.ahead.push(here);
        }
        self.put_back(to);
    }

    pub(super) fn can_go(&self, forward: bool) -> bool {
        if forward {
            !self.ahead.is_empty()
        } else {
            !self.back.is_empty()
        }
    }

    fn put_back(&mut self, spot: Spot) {
        self.menu = None;
        match spot.mode {
            Mode::Files => {
                if self.mode != Mode::Files {
                    self.mode = Mode::Files;
                    self.keep_mode();
                }
                self.files_back(spot.files);
            }
            Mode::Story => {
                if self.mode != Mode::Story {
                    self.open_story_view();
                }
                if self.mode == Mode::Story {
                    let (item, cur, top) = spot.reader;
                    self.reader.item = item;
                    self.reader.cur = cur;
                    self.reader.top = top;
                    self.focus = spot.focus;
                }
            }
            Mode::Map => {
                if self.mode != Mode::Map {
                    self.open_map_view();
                }
                if self.mode == Mode::Map {
                    self.mapper = spot.mapper;
                    self.focus = spot.focus;
                }
            }
        }
    }
}
