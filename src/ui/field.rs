//! The text field every box that takes typing shares (the ask box, the
//! answer box, the reminder box, the menus' and pickers' searches, M-F's
//! query), so each types the same way, as a text box does: what is typed
//! and a cursor in it. New with the port; the boxes each kept a string
//! typed at its end before, with no cursor to move.
//!
//! Left and Right move a character, Alt-Left and Alt-Right (Alt-b, Alt-f) a
//! word, Home and End (C-a, C-e) to the line's start and end, Up and Down a
//! wrapped row, handing the key back at the top or bottom row so a box
//! with rows of its own above or below moves there. Backspace and Delete
//! take a character, Alt-Backspace and C-w the word before, C-u to the
//! line's start, C-k to its end; C-z undoes and C-S-z redoes, a word typed
//! at a time. A multi-line field takes S-Enter and M-Enter as a new line
//! and keeps a paste's new lines; a one-line field ignores S-Enter and
//! joins a paste's lines with spaces. Enter is always the box's.
//!
//! Drawn, it wraps at its width, at a space where one is, measured with
//! `draw::width` so a character a terminal draws two columns wide counts
//! two, keeping a column for the cursor at a row's end; with more rows than
//! the box gives it, it scrolls to keep the cursor in sight. A search's
//! chip has one row, which scrolls sideways instead (`one_row`).

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use unicode_segmentation::UnicodeSegmentation;

use super::draw::width;
use crate::mux::input::{Key, KeyCode, Mods};

/// The field's keys, as mission control's Keys view lists them under
/// "Boxes" (`mission::keymap`).
pub const KEYS: &[(&str, &str)] = &[
    ("← →", "a character back or on"),
    ("M-← M-→ (M-b M-f)", "a word back or on"),
    ("Home End (C-a C-e)", "the line's start or end"),
    ("↑ ↓", "a wrapped row up or down, else the box's list"),
    ("BSpace Delete", "the character before or after the cursor"),
    ("M-BSpace C-w", "the word before the cursor"),
    ("C-u C-k", "to the line's start, or to its end"),
    ("C-z C-S-z", "undo, redo"),
    (
        "S-Enter M-Enter",
        "a new line, where the box takes more than one",
    ),
    ("Enter", "takes what is typed"),
    ("click", "puts the cursor there"),
];

/// What a key did to a field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// The text changed.
    Changed,
    /// The cursor moved.
    Moved,
    /// The field took the key and nothing changed (S-Enter in a one-line
    /// field, Left at its start).
    Same,
    /// Up on its top row: the box above it, if it has one.
    Up,
    /// Down on its bottom row: the box below it, if it has one.
    Down,
    /// Not the field's: the box's (Enter, Esc, Tab).
    Not,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Field {
    text: String,
    /// A byte offset into `text`, on a grapheme's edge.
    cursor: usize,
    multi: bool,
    undo: Vec<(String, usize)>,
    redo: Vec<(String, usize)>,
    /// Whether the last change was a character typed, so the next one
    /// joins its undo step until a space ends the word.
    typing: bool,
    /// The column Up and Down keep while they move.
    goal: Option<usize>,
    /// The width `view` last drew it at, which Up and Down move in; 0 for
    /// a field drawn on one row.
    cols: usize,
    /// The first wrapped row shown.
    top: usize,
}

impl Field {
    pub fn new() -> Field {
        Field::default()
    }

    /// A field that takes new lines (S-Enter, M-Enter, a paste's).
    pub fn multi_line(mut self) -> Field {
        self.multi = true;
        self
    }

    /// What it starts with, the cursor at its end, kept as it is (a shell
    /// command's tabs and lines), so taking it unchanged gives back the
    /// same text; only what is typed or pasted is cleaned.
    pub fn with_text(mut self, text: impl Into<String>) -> Field {
        self.text = text.into();
        self.cursor = self.text.len();
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn is_multi_line(&self) -> bool {
        self.multi
    }

    /// The cursor, as a byte offset into the text.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Puts the cursor at byte `at`, or the nearest character's edge
    /// before it.
    pub fn set_cursor(&mut self, at: usize) {
        let mut at = at.min(self.text.len());
        while !self.text.is_char_boundary(at) {
            at -= 1;
        }
        self.cursor = at;
        self.goal = None;
        self.typing = false;
    }

    /// Puts `text` in place of what is there and the cursor at byte `at`,
    /// starting what C-z undoes afresh: for a box that takes part of what is
    /// typed as something else (M-F's source prefixes), so no step to undo
    /// brings the part back as text.
    pub fn take_in(&mut self, text: &str, at: usize) {
        self.text = self.clean(text);
        self.set_cursor(at);
        self.undo.clear();
        self.redo.clear();
    }

    /// Puts `text` in place of what is there, the cursor at its end, as one
    /// step to undo.
    pub fn set(&mut self, text: &str) {
        let text = self.clean(text);
        if text != self.text {
            self.remember(false);
            self.text = text;
        }
        self.cursor = self.text.len();
        self.goal = None;
    }

    /// Text pasted goes in at the cursor, its new lines kept in a
    /// multi-line field and spaces in a one-line one.
    pub fn paste(&mut self, text: &str) {
        let text = self.clean(text);
        if text.is_empty() {
            return;
        }
        self.remember(false);
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.goal = None;
    }

    /// What a field keeps of `text`: new lines as they are in a multi-line
    /// field (`\r\n` and `\r` as one), else a space each, a tab a space,
    /// and no other control character.
    fn clean(&self, text: &str) -> String {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        text.chars()
            .filter_map(|c| match c {
                '\n' if self.multi => Some('\n'),
                '\n' | '\t' => Some(' '),
                c if c.is_control() => None,
                c => Some(c),
            })
            .collect()
    }

    pub fn key(&mut self, key: Key) -> Edit {
        let k = key.normalized();
        let m = k.mods;
        let plain = m == Mods::NONE;
        let ctrl = m == Mods::CTRL;
        let alt = m == Mods::ALT;
        let vertical = matches!(k.code, KeyCode::Up | KeyCode::Down) && plain;
        if !vertical {
            self.goal = None;
        }
        match k.code {
            KeyCode::Enter if m.shift || m.alt => {
                if !self.multi {
                    return Edit::Same;
                }
                self.insert("\n")
            }
            KeyCode::Char(c) if !m.ctrl && !m.alt => {
                // A word and the spaces after it are one step to undo.
                let starts_word = c != ' ' && self.text[..self.cursor].ends_with(' ');
                let joins = self.typing && !starts_word;
                if !joins {
                    self.remember(true);
                }
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                self.redo.clear();
                self.typing = true;
                Edit::Changed
            }
            KeyCode::Left if plain => self.move_to(self.prev_grapheme(self.cursor)),
            KeyCode::Right if plain => self.move_to(self.next_grapheme(self.cursor)),
            KeyCode::Left if alt => self.move_to(self.word_back()),
            KeyCode::Char('b') if alt => self.move_to(self.word_back()),
            KeyCode::Right if alt => self.move_to(self.word_forward()),
            KeyCode::Char('f') if alt => self.move_to(self.word_forward()),
            KeyCode::Home if plain => self.move_to(self.line_start()),
            KeyCode::Char('a') if ctrl => self.move_to(self.line_start()),
            KeyCode::End if plain => self.move_to(self.line_end()),
            KeyCode::Char('e') if ctrl => self.move_to(self.line_end()),
            KeyCode::Up if plain => self.vertical(false),
            KeyCode::Down if plain => self.vertical(true),
            KeyCode::Backspace if plain || m == Mods::SHIFT => {
                self.delete(self.prev_grapheme(self.cursor), self.cursor)
            }
            KeyCode::Delete if plain => self.delete(self.cursor, self.next_grapheme(self.cursor)),
            KeyCode::Backspace if alt || ctrl => self.delete(self.word_back(), self.cursor),
            KeyCode::Char('w') if ctrl => self.delete(self.word_back(), self.cursor),
            KeyCode::Char('u') if ctrl => self.delete(self.line_start(), self.cursor),
            KeyCode::Char('k') if ctrl => self.delete(self.cursor, self.line_end()),
            KeyCode::Char('z') if ctrl => self.restore(true),
            KeyCode::Char('z') if m.ctrl && m.shift && !m.alt => self.restore(false),
            _ => Edit::Not,
        }
    }

    fn insert(&mut self, text: &str) -> Edit {
        self.remember(false);
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
        Edit::Changed
    }

    fn delete(&mut self, from: usize, to: usize) -> Edit {
        if from >= to {
            return Edit::Same;
        }
        self.remember(false);
        self.text.replace_range(from..to, "");
        self.cursor = from;
        Edit::Changed
    }

    fn move_to(&mut self, to: usize) -> Edit {
        self.typing = false;
        if to == self.cursor {
            return Edit::Same;
        }
        self.cursor = to;
        Edit::Moved
    }

    /// Keeps the text as it is before a change, for C-z.
    fn remember(&mut self, typing: bool) {
        self.undo.push((self.text.clone(), self.cursor));
        self.redo.clear();
        self.typing = typing;
    }

    fn restore(&mut self, undo: bool) -> Edit {
        let (from, to) = if undo {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        let Some((text, cursor)) = from.pop() else {
            return Edit::Same;
        };
        to.push((std::mem::replace(&mut self.text, text), self.cursor));
        self.cursor = cursor;
        self.typing = false;
        Edit::Changed
    }

    fn prev_grapheme(&self, at: usize) -> usize {
        self.text[..at]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next_grapheme(&self, at: usize) -> usize {
        self.text[at..]
            .graphemes(true)
            .next()
            .map_or(at, |g| at + g.len())
    }

    /// The start of the word before the cursor, past the spaces before it.
    fn word_back(&self) -> usize {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end_matches(char::is_whitespace);
        trimmed
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(i, c)| i + c.len_utf8())
    }

    /// The end of the word after the cursor, past the spaces before it.
    fn word_forward(&self) -> usize {
        let after = &self.text[self.cursor..];
        let spaces = after.len() - after.trim_start_matches(char::is_whitespace).len();
        let rest = &after[spaces..];
        let word = rest.find(char::is_whitespace).unwrap_or(rest.len());
        self.cursor + spaces + word
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i)
    }

    /// The wrapped rows at `cols` columns, each a range of the text: a row
    /// ends where the next starts, or at its line's new line.
    pub fn rows(&self, cols: usize) -> Vec<(usize, usize)> {
        // A column kept for the cursor after a row's last character.
        let room = cols.saturating_sub(1).max(1);
        let mut rows = Vec::new();
        let mut base = 0;
        for line in self.text.split('\n') {
            let mut start = 0;
            loop {
                let rest = &line[start..];
                if width(rest) <= room {
                    rows.push((base + start, base + line.len()));
                    break;
                }
                let mut used = 0;
                let mut end = start;
                let mut after_space = None;
                for (i, g) in rest.grapheme_indices(true) {
                    let w = width(g);
                    if used + w > room {
                        break;
                    }
                    used += w;
                    end = start + i + g.len();
                    if g == " " {
                        after_space = Some(end);
                    }
                }
                let next = if line[end..].starts_with(' ') {
                    // A space at the edge hangs in the cursor's column.
                    end + 1
                } else if let Some(at) = after_space {
                    at
                } else if end > start {
                    end
                } else {
                    // A character wider than the row goes on one alone.
                    start + rest.graphemes(true).next().map_or(rest.len(), str::len)
                };
                rows.push((base + start, base + next));
                start = next;
                if start >= line.len() {
                    // The cursor after a space hanging at the edge goes
                    // on a row of its own.
                    rows.push((base + start, base + start));
                    break;
                }
            }
            base += line.len() + 1;
        }
        rows
    }

    /// The row the cursor is on, and its column there.
    fn place(&self, rows: &[(usize, usize)]) -> (usize, usize) {
        let row = rows
            .iter()
            .rposition(|(start, _)| *start <= self.cursor)
            .unwrap_or(0);
        let start = rows.get(row).map_or(0, |r| r.0);
        (row, width(&self.text[start..self.cursor]))
    }

    /// The offset in row `row` nearest column `col`, before the row's end
    /// where the next row starts at it.
    fn offset_in(&self, rows: &[(usize, usize)], row: usize, col: usize) -> usize {
        let (start, end) = rows[row];
        let soft = rows.get(row + 1).is_some_and(|next| next.0 == end);
        let mut at = start;
        let mut used = 0;
        for (i, g) in self.text[start..end].grapheme_indices(true) {
            let w = width(g);
            if used + w > col {
                break;
            }
            let next = start + i + g.len();
            if soft && next == end {
                break;
            }
            used += w;
            at = next;
        }
        at
    }

    fn vertical(&mut self, down: bool) -> Edit {
        // Never drawn in rows (a search's chip, `one_row`): one row.
        if self.cols == 0 {
            return if down { Edit::Down } else { Edit::Up };
        }
        let rows = self.rows(self.cols);
        let (row, col) = self.place(&rows);
        let to = if down {
            if row + 1 >= rows.len() {
                return Edit::Down;
            }
            row + 1
        } else {
            if row == 0 {
                return Edit::Up;
            }
            row - 1
        };
        let goal = *self.goal.get_or_insert(col);
        self.cursor = self.offset_in(&rows, to, goal);
        self.typing = false;
        Edit::Moved
    }

    /// Its rows to draw at `cols` columns, at most `most`, scrolled to keep
    /// the cursor in sight: the cursor on the character it is before,
    /// reversed, or a `▏` in `cursor_style` at a row's end, while `cursor`.
    pub fn view(
        &mut self,
        cols: usize,
        most: usize,
        style: Style,
        cursor_style: Style,
        cursor: bool,
    ) -> Vec<Vec<Span<'static>>> {
        self.cols = cols;
        let rows = self.rows(cols);
        let most = most.max(1);
        let (at, _) = self.place(&rows);
        if at < self.top {
            self.top = at;
        } else if at >= self.top + most {
            self.top = at + 1 - most;
        }
        self.top = self.top.min(rows.len().saturating_sub(most));
        rows.iter()
            .enumerate()
            .skip(self.top)
            .take(most)
            .map(|(i, &(start, end))| {
                let text = self.text[start..end].trim_end_matches('\n');
                self.row_spans(text, start, i == at && cursor, style, cursor_style)
            })
            .collect()
    }

    /// One row's spans, the cursor drawn in it when it is there.
    fn row_spans(
        &self,
        text: &str,
        start: usize,
        here: bool,
        style: Style,
        cursor_style: Style,
    ) -> Vec<Span<'static>> {
        // A tab kept in the text shows as a space, the same length, so
        // the offsets hold.
        let text = &text.replace('\t', " ");
        if !here {
            return vec![Span::styled(text.to_string(), style)];
        }
        let at = (self.cursor - start).min(text.len());
        let (before, after) = text.split_at(at);
        let mut spans = vec![Span::styled(before.to_string(), style)];
        match after.graphemes(true).next() {
            Some(g) => {
                spans.push(Span::styled(
                    g.to_string(),
                    style.add_modifier(Modifier::REVERSED),
                ));
                spans.push(Span::styled(after[g.len()..].to_string(), style));
            }
            _ => spans.push(Span::styled("▏", cursor_style)),
        }
        spans.retain(|s| !s.content.is_empty());
        spans
    }

    /// The rows `view` last showed, by their place there, and a click on
    /// one at a column: the cursor goes there.
    pub fn click(&mut self, row: usize, col: usize) {
        let rows = self.rows(self.cols);
        let Some(i) = rows
            .len()
            .checked_sub(1)
            .map(|last| (self.top + row).min(last))
        else {
            return;
        };
        self.cursor = self.offset_in(&rows, i, col);
        self.typing = false;
        self.goal = None;
    }

    /// The one row a search's chip shows, `cols` wide: as much of the text
    /// as fits, scrolled sideways to keep the cursor in sight, the cursor
    /// drawn as `view` draws it. New lines show as spaces.
    pub fn one_row(&self, cols: usize, style: Style, cursor_style: Style) -> Vec<Span<'static>> {
        let room = cols.saturating_sub(1).max(1);
        let text = self.text.replace('\n', " ");
        let start = self.one_row_start(room);
        let mut end = start;
        let mut used = 0;
        for g in text[start..].graphemes(true) {
            if used + width(g) > room {
                break;
            }
            used += width(g);
            end += g.len();
        }
        self.row_spans(&text[start..end], start, true, style, cursor_style)
    }

    /// Where the row `one_row` shows in `room` columns starts.
    fn one_row_start(&self, room: usize) -> usize {
        let mut start = 0;
        while width(&self.text[start..self.cursor]) > room {
            start = self.next_grapheme(start);
        }
        start
    }

    /// A click at column `col` of the row `one_row` showed at `cols`
    /// columns: the cursor goes there.
    pub fn click_one_row(&mut self, cols: usize, col: usize) {
        let mut at = self.one_row_start(cols.saturating_sub(1).max(1));
        let mut used = 0;
        for g in self.text[at..].graphemes(true) {
            if used + width(g) > col {
                break;
            }
            used += width(g);
            at += g.len();
        }
        self.cursor = at;
        self.typing = false;
        self.goal = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(f: &mut Field, code: KeyCode, mods: Mods) -> Edit {
        f.key(Key::new(code, mods))
    }

    fn typed(f: &mut Field, text: &str) {
        for c in text.chars() {
            press(f, KeyCode::Char(c), Mods::NONE);
        }
    }

    fn at(f: &Field) -> (&str, &str) {
        f.text().split_at(f.cursor())
    }

    fn shown(f: &mut Field, cols: usize, most: usize) -> Vec<String> {
        f.view(cols, most, Style::new(), Style::new(), true)
            .iter()
            .map(|row| row.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    const CTRL: Mods = Mods::CTRL;
    const ALT: Mods = Mods::ALT;
    const NONE: Mods = Mods::NONE;

    #[test]
    fn it_moves_by_character_word_and_line() {
        let mut f = Field::new().with_text("one two three");
        assert_eq!(press(&mut f, KeyCode::Left, NONE), Edit::Moved);
        assert_eq!(at(&f), ("one two thre", "e"));
        press(&mut f, KeyCode::Left, ALT);
        assert_eq!(at(&f), ("one two ", "three"));
        press(&mut f, KeyCode::Char('b'), ALT);
        assert_eq!(at(&f), ("one ", "two three"));
        press(&mut f, KeyCode::Right, ALT);
        assert_eq!(at(&f), ("one two", " three"));
        press(&mut f, KeyCode::Char('f'), ALT);
        assert_eq!(at(&f), ("one two three", ""));
        press(&mut f, KeyCode::Home, NONE);
        assert_eq!(f.cursor(), 0);
        assert_eq!(press(&mut f, KeyCode::Left, NONE), Edit::Same);
        press(&mut f, KeyCode::Right, NONE);
        assert_eq!(at(&f), ("o", "ne two three"));
        press(&mut f, KeyCode::Char('e'), CTRL);
        assert_eq!(f.cursor(), f.text().len());
        press(&mut f, KeyCode::Char('a'), CTRL);
        assert_eq!(f.cursor(), 0);
        press(&mut f, KeyCode::End, NONE);
        assert_eq!(f.cursor(), f.text().len());
        typed(&mut f, "!");
        assert_eq!(f.text(), "one two three!");
    }

    #[test]
    fn it_types_and_deletes_where_the_cursor_is() {
        let mut f = Field::new().with_text("helo");
        press(&mut f, KeyCode::Left, NONE);
        typed(&mut f, "l");
        assert_eq!(f.text(), "hello");
        press(&mut f, KeyCode::Delete, NONE);
        assert_eq!(f.text(), "hell");
        press(&mut f, KeyCode::Backspace, NONE);
        assert_eq!(at(&f), ("hel", ""));
        let mut f = Field::new().with_text("one two three");
        press(&mut f, KeyCode::Char('w'), CTRL);
        assert_eq!(f.text(), "one two ");
        press(&mut f, KeyCode::Backspace, ALT);
        assert_eq!(f.text(), "one ");
        let mut f = Field::new().with_text("one two three");
        for _ in 0..5 {
            press(&mut f, KeyCode::Left, NONE);
        }
        press(&mut f, KeyCode::Char('k'), CTRL);
        assert_eq!(f.text(), "one two ");
        press(&mut f, KeyCode::Left, NONE);
        press(&mut f, KeyCode::Char('u'), CTRL);
        assert_eq!(at(&f), ("", " "));
    }

    #[test]
    fn the_line_keys_keep_to_the_line_the_cursor_is_on() {
        let mut f = Field::new().multi_line().with_text("first\nsecond");
        press(&mut f, KeyCode::Left, NONE);
        press(&mut f, KeyCode::Home, NONE);
        assert_eq!(at(&f), ("first\n", "second"));
        press(&mut f, KeyCode::Left, NONE);
        press(&mut f, KeyCode::Char('u'), CTRL);
        assert_eq!(f.text(), "\nsecond");
    }

    #[test]
    fn undo_takes_back_a_word_at_a_time_and_redo_puts_it_back() {
        let mut f = Field::new();
        typed(&mut f, "one two");
        assert_eq!(press(&mut f, KeyCode::Char('z'), CTRL), Edit::Changed);
        assert_eq!(f.text(), "one ");
        press(&mut f, KeyCode::Char('z'), CTRL);
        assert_eq!(f.text(), "");
        assert_eq!(press(&mut f, KeyCode::Char('z'), CTRL), Edit::Same);
        press(
            &mut f,
            KeyCode::Char('z'),
            Mods {
                shift: true,
                ..CTRL
            },
        );
        assert_eq!(f.text(), "one ");
        press(&mut f, KeyCode::Backspace, NONE);
        assert_eq!(f.text(), "one");
        press(&mut f, KeyCode::Char('z'), CTRL);
        assert_eq!(f.text(), "one ");
    }

    #[test]
    fn it_wraps_at_a_space_and_inside_a_long_word() {
        let mut f = Field::new().with_text("the quick brown fox");
        // Ten columns: nine for text and one for the cursor.
        assert_eq!(shown(&mut f, 10, 9), vec!["the quick ", "brown fox▏"]);
        let mut f = Field::new().with_text("abcdefghijklmnop");
        assert_eq!(shown(&mut f, 6, 9), vec!["abcde", "fghij", "klmno", "p▏"]);
    }

    #[test]
    fn two_column_characters_count_as_two() {
        let mut f = Field::new().with_text("日本語のテキスト");
        assert_eq!(shown(&mut f, 7, 9), vec!["日本語", "のテキ", "スト▏"]);
        press(&mut f, KeyCode::Home, NONE);
        press(&mut f, KeyCode::Right, NONE);
        assert_eq!(at(&f), ("日", "本語のテキスト"));
    }

    #[test]
    fn up_and_down_move_a_wrapped_row_and_hand_back_at_the_edges() {
        let mut f = Field::new().with_text("the quick brown fox");
        shown(&mut f, 10, 9);
        press(&mut f, KeyCode::Left, NONE);
        press(&mut f, KeyCode::Left, NONE);
        assert_eq!(press(&mut f, KeyCode::Up, NONE), Edit::Moved);
        assert_eq!(at(&f), ("the qui", "ck brown fox"));
        assert_eq!(press(&mut f, KeyCode::Up, NONE), Edit::Up);
        assert_eq!(press(&mut f, KeyCode::Down, NONE), Edit::Moved);
        assert_eq!(at(&f), ("the quick brown f", "ox"));
        assert_eq!(press(&mut f, KeyCode::Down, NONE), Edit::Down);
        let mut one = Field::new().with_text("short");
        shown(&mut one, 20, 1);
        assert_eq!(press(&mut one, KeyCode::Down, NONE), Edit::Down);
    }

    #[test]
    fn it_scrolls_to_keep_the_cursor_in_sight() {
        let mut f = Field::new().with_text("aaaa bbbb cccc dddd");
        assert_eq!(shown(&mut f, 6, 2), vec!["cccc ", "dddd▏"]);
        press(&mut f, KeyCode::Home, NONE);
        assert_eq!(shown(&mut f, 6, 2)[0], "aaaa ");
    }

    #[test]
    fn s_enter_adds_a_line_only_in_a_multi_line_field() {
        let mut f = Field::new().multi_line();
        typed(&mut f, "one");
        assert_eq!(press(&mut f, KeyCode::Enter, Mods::SHIFT), Edit::Changed);
        press(&mut f, KeyCode::Enter, ALT);
        typed(&mut f, "two");
        assert_eq!(f.text(), "one\n\ntwo");
        assert_eq!(shown(&mut f, 20, 9), vec!["one", "", "two▏"]);
        assert_eq!(press(&mut f, KeyCode::Enter, NONE), Edit::Not);
        let mut f = Field::new().with_text("one");
        assert_eq!(press(&mut f, KeyCode::Enter, Mods::SHIFT), Edit::Same);
        assert_eq!(f.text(), "one");
    }

    #[test]
    fn a_paste_keeps_its_lines_only_in_a_multi_line_field() {
        let mut f = Field::new().multi_line().with_text("ab");
        press(&mut f, KeyCode::Left, NONE);
        f.paste("1\r\n2\t3");
        assert_eq!(f.text(), "a1\n2 3b");
        assert_eq!(at(&f), ("a1\n2 3", "b"));
        let mut f = Field::new();
        f.paste("1\n2\r3");
        assert_eq!(f.text(), "1 2 3");
    }

    #[test]
    fn a_click_puts_the_cursor_where_it_lands() {
        let mut f = Field::new().with_text("the quick brown fox");
        shown(&mut f, 10, 9);
        f.click(1, 2);
        assert_eq!(at(&f), ("the quick br", "own fox"));
        f.click(1, 40);
        assert_eq!(f.cursor(), f.text().len());
        f.click(0, 40);
        assert_eq!(at(&f), ("the quick", " brown fox"));
    }

    #[test]
    fn the_cursor_shows_on_the_character_it_is_before() {
        let mut f = Field::new().with_text("abc");
        press(&mut f, KeyCode::Left, NONE);
        let row = &f.view(10, 1, Style::new(), Style::new(), true)[0];
        assert_eq!(row[1].content, "c");
        assert!(row[1].style.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn a_kept_tab_counts_as_the_space_it_shows_as() {
        assert_eq!(width("a\tb"), 3);
        let mut f = Field::new().with_text("a\tb");
        shown(&mut f, 10, 1);
        f.click(0, 2);
        assert_eq!(at(&f), ("a\t", "b"));
    }

    #[test]
    fn one_row_scrolls_sideways_to_the_cursor() {
        let f = Field::new().with_text("abcdefghij");
        let text: String = f
            .one_row(5, Style::new(), Style::new())
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(text, "ghij▏");
        let mut f = f;
        f.click_one_row(5, 1);
        assert_eq!(at(&f), ("abcdefg", "hij"));
    }
}
