//! The file's sides read with the shared Vim (`switchboard_vim`), read-only,
//! as the code viewer reads: its motions, counts and marks over the shown
//! file's screen lines on the side with the focus; a key that would change
//! the text says Vim's E21. The diff's own keys come first: its pairs (`]c`,
//! `gt`, `zo`, `y`...), `v` and `V`, Left and Right, Enter, Ctrl-E and Ctrl-Y.

use switchboard_ui::draw::width;
use switchboard_ui::input::Key;
use switchboard_ui::kit::nav::{self, Move};
use switchboard_vim::{Effect, Mode, Text};

use super::{Act, App, Cell, Focus, Line};

impl App {
    /// The shown file's screen lines on one side, as Vim reads them: a
    /// pair's piece of its line, a gap or a fold empty, a card or an "also
    /// in" line its words.
    pub(super) fn side_text(&mut self, new_side: bool) -> Vec<String> {
        let lines = self.screen_lines().to_vec();
        let Some(sides) = self.current().and_then(|e| self.files.get(&e.key())) else {
            return Vec::new();
        };
        let side = if new_side { &sides.new } else { &sides.old };
        let text = |l: &Line| match l {
            Line::Pair { old, new, .. } => match if new_side { new } else { old } {
                Cell::Text { line, range } => side[*line][range.clone()].to_string(),
                Cell::Gap => String::new(),
            },
            Line::Fold { .. } => String::new(),
            Line::Card(spans) | Line::Also(_, _, spans) => {
                spans.iter().map(|s| s.content.as_ref()).collect()
            }
        };
        lines.iter().map(text).collect()
    }

    /// Whether the side with the focus has a Vim command half typed (`f`
    /// waiting for its character, `m` for its mark), which takes the next
    /// key whatever it is.
    pub(super) fn vim_waiting(&self) -> bool {
        self.focus != Focus::List && self.vim.pending()
    }

    /// A screen of the shown file from the list (Ctrl-F, Ctrl-B and the
    /// page keys), moved on its side as there, the list keeping the keys.
    pub(super) fn page_from_list(&mut self, key: &crossterm::event::KeyEvent) -> Act {
        let Some(key) = Key::of_crossterm(key) else {
            return Act::Same;
        };
        self.focus = self.main_side();
        let act = self.vim_keys(&[key], 1);
        self.focus = Focus::List;
        act
    }

    /// `keys` on the side with the focus, `count` times: the cursor goes
    /// where Vim takes it and the view follows.
    pub(super) fn vim_keys(&mut self, keys: &[Key], count: usize) -> Act {
        let text = self.side_text(self.focus == Focus::New);
        let Some(last) = text.len().checked_sub(1) else {
            return Act::Same;
        };
        let at_end = self.cur >= last && self.at + 1 < self.order.len();
        let row = self.cur.min(last);
        let byte = byte_at(&text[row], self.col);
        let text = Text::from_lines(&text);
        if self.vim.doc.text != text {
            self.vim.change_outside(|doc| doc.text = text);
        }
        self.vim.go_to(row, byte);
        self.vim.top = if self.view == self.at { self.top } else { 0 };
        self.vim.height = self.body_height().max(1);
        self.vim.begin();
        if count > 1 {
            for digit in count.to_string().chars() {
                self.vim
                    .key(Key::plain(switchboard_ui::input::KeyCode::Char(digit)));
            }
        }
        for key in keys {
            self.vim.key(*key);
        }
        self.vim.end();
        for effect in self.vim.effects() {
            if let Effect::Message(words) = effect {
                self.say(words);
            }
        }
        // The diff's own `v` selects; Vim's would show nowhere.
        if matches!(self.vim.mode, Mode::Visual { .. }) {
            self.vim.leave_visual();
        }
        self.vim.center = false;
        let (to, byte) = (self.vim.row(), self.vim.col());
        let line = self.vim.line();
        let down = keys.last().copied().and_then(nav::step) == Some(Move::Down);
        if down && to == row && at_end && !self.said_end {
            self.said_end = true;
            let path = self.current().map(|e| e.path.clone()).unwrap_or_default();
            let name = path.rsplit('/').next().unwrap_or_default();
            self.say(format!("End of {name} · ]f for the next file"));
        }
        self.adrift = false;
        self.cur = to;
        self.col = width(&line[..byte.min(line.len())]);
        self.moved = true;
        self.keep_cursor();
        Act::Redraw
    }
}

/// The byte of `line` at display column `col`, its end past it.
fn byte_at(line: &str, col: usize) -> usize {
    line.char_indices()
        .find(|(i, _)| width(&line[..*i]) >= col)
        .map_or(line.len(), |(i, _)| i)
}

/// `g`, for `gg`.
pub(super) fn g_key() -> Key {
    Key::plain(input_code('g'))
}

/// A character as the shared Vim's keys name it.
pub(super) fn input_code(c: char) -> switchboard_ui::input::KeyCode {
    switchboard_ui::input::KeyCode::Char(c)
}
