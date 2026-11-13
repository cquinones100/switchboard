//! Vim's keys on a note, in place of the Neovim the plugin ran in: Normal,
//! Insert, Replace and Visual modes, motions, operators and text objects, counts,
//! registers, undo and redo, and search, enough of Vim that a note edits
//! the way it did. What a note added on top (Enter ticking a box, the Format
//! menu, the leader keys) is the app's; this is the editing underneath.
//!
//! The settings are the ones nvim/init.lua gave every buffer: Space as the
//! leader (taken by the app), `shiftwidth` 2 with spaces, `autoindent`,
//! `ignorecase` and `smartcase` for search, and `clipboard=unnamed`, so a
//! yank or a delete reaches the system clipboard and `p` reads it back.
//!
//! An edit is undone as Vim undoes it: a Normal mode command at a time, and
//! everything typed from entering Insert mode to leaving it as one. The app
//! calls `begin` before each key and `end` after, and the history keeps the
//! whole text before each change, which a rope keeps for little.

use std::sync::LazyLock;

use regex::Regex;

use super::fold::Folds;
use super::format::{Block, Doc, Insert, Selection};
use super::render;
use super::text::Text;
use crate::mux::input::{Key, KeyCode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    /// `R`: typing replaces the character under the cursor.
    Replace,
    /// A selection: by characters, whole lines or a block (Ctrl-V);
    /// `select` is Select mode (easy mode's), where typing replaces the
    /// selection.
    Visual {
        line: bool,
        select: bool,
        block: bool,
    },
}

/// What a key asks of the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Say something on the bottom line.
    Message(String),
    /// Put text on the system clipboard (a yank or a delete).
    Copy(String),
    Save,
    Quit,
    SaveQuit,
    /// Ask for a search pattern (`/` forward, `?` back) or an Ex command.
    Prompt(char),
    /// Ctrl-W and a direction: to the list (`h`) or the note (`l`).
    Window(char),
    /// `gw`: start work on the checklist item on the cursor's line in a
    /// new tab.
    WorkOnItem,
    /// `ga`: UX 2.0's "Add an item" input at the bottom, typed into.
    AddItem,
}

/// A register's text, and whether it holds whole lines or a block (a
/// line of text for each row, put as a column).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Register {
    pub text: String,
    pub linewise: bool,
    pub block: bool,
}

/// Typing into every line of a block (`I`, `A`, `c` on one): its rows,
/// the character column the typing goes in at on each (each line's end
/// with `at_end`), the rows passed over since they didn't reach into the
/// block (`I` and `c`; `A` pads a short one with spaces instead), and
/// the byte on the first row the typing starts.
#[derive(Clone, Debug, PartialEq, Eq)]
struct BlockInsert {
    rows: (usize, usize),
    /// The row typed on: the first that isn't passed over.
    row: usize,
    /// Where the last key typed left the cursor: moved from there any
    /// other way (a click), the typing goes into this row alone.
    at: (usize, usize),
    col: usize,
    at_end: bool,
    skip: Vec<usize>,
    from: usize,
}

/// What a key typed in Replace mode did, for Backspace to take back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Over {
    /// Typed over this character.
    Char(char),
    /// Typed past the line's end.
    Added,
    /// Enter broke the line.
    Break,
}

/// A key Insert mode waits for after Ctrl-R, or the Normal mode command
/// it runs after Ctrl-O, from where the cursor was (past the line's end
/// too, which the command's cursor can't be).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Waiting {
    Register,
    OneCommand { at: (usize, usize) },
}

/// The text before a change, to undo to.
#[derive(Clone, Debug)]
struct Snap {
    text: Text,
    row: usize,
    col: usize,
}

/// A note's history and folds while another is open (`Editor::put_away`).
#[derive(Clone, Debug)]
pub struct Kept {
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    folds: Folds,
    text: Text,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Motion {
    Left,
    Right,
    Down,
    Up,
    Word { big: bool },
    Back { big: bool },
    End { big: bool },
    EndBack { big: bool },
    LineStart,
    FirstNonBlank,
    LineEnd,
    Goto { first: bool },
    Find { c: char, back: bool, till: bool },
    RepeatFind { reverse: bool },
    ParagraphFwd,
    ParagraphBack,
    Match,
    Screen(u8),
    Next { reverse: bool },
    NextLine,
    PrevLine,
    CurrentLine,
}

impl Motion {
    fn linewise(self) -> bool {
        matches!(
            self,
            Motion::Down
                | Motion::Up
                | Motion::Goto { .. }
                | Motion::Screen(_)
                | Motion::NextLine
                | Motion::PrevLine
                | Motion::CurrentLine
        )
    }

    fn inclusive(self) -> bool {
        matches!(
            self,
            Motion::End { .. }
                | Motion::EndBack { .. }
                | Motion::LineEnd
                | Motion::Find {
                    till: _,
                    back: false,
                    ..
                }
                | Motion::Match
        ) || matches!(self, Motion::RepeatFind { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
    Toggle,
    Lower,
    Upper,
}

/// A range to act on: from a position to another (the end exclusive),
/// whole lines, or a block of rows and columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Range {
    Chars((usize, usize), (usize, usize)),
    Lines(usize, usize),
    Block((usize, usize), Block),
}

/// What a complete Normal mode command is.
enum Parse {
    More,
    Invalid,
    Done(Command),
}

enum Target {
    Motion(Motion),
    Object { inner: bool, what: char },
    Same,
}

enum Command {
    Move(Motion, usize),
    Operate(Op, usize, Target),
    Simple(char, usize),
    Two(char, char, usize),
    Replace(char, usize),
}

pub struct Editor {
    pub doc: Doc,
    pub mode: Mode,
    /// Where a selection started.
    pub anchor: (usize, usize),
    /// Keys of a Normal mode command not yet complete.
    keys: Vec<Key>,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    group: Option<Snap>,
    pub register: Register,
    /// Through the system clipboard (`clipboard=unnamed`); off in the tests.
    pub clipboard: bool,
    last_find: Option<(char, bool, bool)>,
    pub search: Option<(String, bool)>,
    /// The column j and k try to keep, in characters.
    want: Option<usize>,
    pub folds: Folds,
    /// The first row on the screen and how many the window has, for H, M,
    /// L and the scrolling keys; the app keeps them.
    pub top: usize,
    pub height: usize,
    /// `zz` asked for the cursor's line in the middle of the screen, which
    /// the app does as it draws, knowing how many rows each line wraps to.
    pub center: bool,
    /// Easy mode: Shift with an arrow starts a selection, and typing over
    /// one replaces it.
    pub easy: bool,
    /// Replace mode: what each key typed since `R` (or since the cursor
    /// last moved) did, what it typed, and `R`'s count, which types it
    /// again on Esc.
    over: Vec<Over>,
    typed_over: String,
    over_count: usize,
    /// Where Replace mode's last key left the cursor: moved from there any
    /// other way (a click), what it typed over can't be put back.
    over_end: (usize, usize),
    waiting: Option<Waiting>,
    block_insert: Option<BlockInsert>,
    /// A block's cursor column when `$` stretched it to each line's end,
    /// which its left edge keeps.
    end_from: usize,
    /// A block's anchor column in characters, kept apart from the anchor's
    /// byte, which a short row clamps, so swapping corners keeps the
    /// rectangle.
    anchor_col: usize,
    out: Vec<Effect>,
}

/// The characters of a line, with each one's byte.
fn chars_of(line: &str) -> Vec<(usize, char)> {
    line.char_indices().collect()
}

fn class(c: char, big: bool) -> u8 {
    if c.is_whitespace() {
        0
    } else if big || !(c.is_alphanumeric() || c == '_') {
        1
    } else {
        2
    }
}

/// The count the keys start with, and how many keys it takes: typed digits
/// not starting with `0`, as `parse` reads one.
fn leading_count(keys: &[Key]) -> (usize, usize) {
    let mut n = 0usize;
    let mut len = 0;
    while let Some(d) = keys.get(len).and_then(typed).and_then(|c| c.to_digit(10)) {
        if d == 0 && len == 0 {
            break;
        }
        n = n.saturating_mul(10).saturating_add(d as usize);
        len += 1;
    }
    (n, len)
}

/// The keys of a change with its count replaced, for a count typed before
/// `.`: the count in front taken off, and an operator's own count too (Vim
/// keeps `2d3w` as one count, which the new one replaces whole), the new
/// count put in front. Nothing for a change whose first key means something
/// else after a count, which `.` then makes again as recorded: a key that
/// isn't a character (Enter's action, a Shift-arrow selection), a register
/// named, a selection started, the leader, or an insert the notes app places
/// after a list's marker, which it does only with nothing pending.
pub fn recount(keys: &[Key], count: usize) -> Option<Vec<Key>> {
    let mut rest = &keys[leading_count(keys).1..];
    let first = rest.first()?;
    let takes_a_count = match typed(first) {
        Some(' ' | 'i' | 'a' | 'I' | '"' | 'v' | 'V') => false,
        Some(_) => true,
        None => ctrl(first, 'a') || ctrl(first, 'x'),
    };
    if !takes_a_count {
        return None;
    }
    let operator = match typed(first) {
        Some('d' | 'c' | 'y' | '>' | '<') => 1,
        Some('g') if matches!(rest.get(1).and_then(typed), Some('~' | 'u' | 'U')) => 2,
        _ => 0,
    };
    let mut out: Vec<Key> = count
        .to_string()
        .chars()
        .map(|d| Key::new(KeyCode::Char(d), crate::mux::input::Mods::NONE))
        .collect();
    if operator > 0 {
        out.extend_from_slice(&rest[..operator]);
        rest = &rest[operator..];
        rest = &rest[leading_count(rest).1..];
    }
    out.extend_from_slice(rest);
    Some(out)
}

/// A key as the character it types, with no Ctrl or Alt.
fn typed(key: &Key) -> Option<char> {
    match key.code {
        KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => Some(c),
        _ => None,
    }
}

fn ctrl(key: &Key, c: char) -> bool {
    key.mods.ctrl
        && !key.mods.alt
        && matches!(key.code, KeyCode::Char(k) if k.eq_ignore_ascii_case(&c))
}

/// The byte range of the number Ctrl-A and Ctrl-X change: the run of digits
/// the cursor is on, else the first after it on the line, with a `-` right
/// before it as its sign when nothing but a blank or the line's start comes
/// before that, as Vim's `nrformats=blank` reads it, so a date's `-11` stays
/// a positive 11.
fn number_at(line: &str, col: usize) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut start = (col.min(bytes.len())..bytes.len()).find(|&i| bytes[i].is_ascii_digit())?;
    while start > 0 && bytes[start - 1].is_ascii_digit() {
        start -= 1;
    }
    let end = (start..bytes.len())
        .find(|&i| !bytes[i].is_ascii_digit())
        .unwrap_or(bytes.len());
    let signed = start > 0
        && bytes[start - 1] == b'-'
        && (start == 1 || bytes[start - 2].is_ascii_whitespace());
    Some((if signed { start - 1 } else { start }, end))
}

/// A number's text with `by` added, keeping the digits' width when it had
/// leading zeros (`007` to `008`), as Vim does; None past what fits.
fn add_to_number(text: &str, by: i64) -> Option<String> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, text),
    };
    let value: i64 = digits.parse().ok()?;
    let value = if negative { -value } else { value };
    let result = value.checked_add(by)?;
    let width = if digits.len() > 1 && digits.starts_with('0') {
        digits.len()
    } else {
        0
    };
    let sign = if result < 0 { "-" } else { "" };
    Some(format!("{sign}{:0width$}", result.unsigned_abs()))
}

fn first_non_blank(line: &str) -> usize {
    line.char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map_or(0, |(i, _)| i)
}

impl Editor {
    pub fn new(text: Text) -> Editor {
        Editor {
            doc: Doc::new(text),
            mode: Mode::Normal,
            anchor: (0, 0),
            keys: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            group: None,
            register: Register::default(),
            clipboard: false,
            last_find: None,
            search: None,
            want: None,
            folds: Folds::default(),
            top: 0,
            height: 20,
            center: false,
            easy: false,
            over: Vec::new(),
            typed_over: String::new(),
            over_count: 1,
            over_end: (0, 0),
            waiting: None,
            block_insert: None,
            end_from: 0,
            anchor_col: 0,
            out: Vec::new(),
        }
    }

    /// Takes the open note's undo and redo history and its folds out, with
    /// the text they end at, to be put back when it opens again (`take_back`),
    /// as Neovim kept a buffer per note; a change under way ends here.
    pub fn put_away(&mut self) -> Kept {
        if let Some(group) = self.group.take()
            && group.text != self.doc.text
        {
            self.undo.push(group);
        }
        Kept {
            undo: std::mem::take(&mut self.undo),
            redo: std::mem::take(&mut self.redo),
            folds: std::mem::take(&mut self.folds),
            text: self.doc.text.clone(),
        }
    }

    /// A note's text put in with the history and folds it was left with:
    /// a change made to its file meanwhile is one more step to undo, as
    /// Neovim's buffer took a file changed under it.
    pub fn take_back(&mut self, text: Text, kept: Kept) {
        self.load(kept.text);
        self.undo = kept.undo;
        self.redo = kept.redo;
        self.folds = kept.folds;
        if text != self.doc.text {
            let before = self.doc.text.lines();
            self.change(|doc| doc.text = text);
            let after = self.doc.text.lines();
            self.folds.follow(&before, &after);
        }
    }

    /// A note's text put in, its history gone.
    pub fn load(&mut self, text: Text) {
        self.doc.text = text;
        self.doc.row = 0;
        self.doc.col = 0;
        self.doc.pending = None;
        self.doc.insert = None;
        self.mode = Mode::Normal;
        self.keys.clear();
        self.waiting = None;
        self.block_insert = None;
        self.undo.clear();
        self.redo.clear();
        self.group = None;
        self.folds = Folds::default();
        self.top = 0;
        self.want = None;
    }

    pub fn effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.out)
    }

    fn say(&mut self, text: impl Into<String>) {
        self.out.push(Effect::Message(text.into()));
    }

    /// Whether a Normal mode command is half typed.
    pub fn idle(&self) -> bool {
        self.keys.is_empty() && self.mode == Mode::Normal && self.waiting.is_none()
    }

    /// Whether keys are waiting for the rest of a command: a Normal mode
    /// one, a register's name after Ctrl-R, or the command after Ctrl-O.
    pub fn pending(&self) -> bool {
        !self.keys.is_empty() || self.waiting.is_some()
    }

    /// Whether Normal mode is running one command for Insert mode (Ctrl-O).
    pub fn one_command(&self) -> bool {
        matches!(self.waiting, Some(Waiting::OneCommand { .. }))
    }

    pub fn row(&self) -> usize {
        self.doc.row
    }

    pub fn col(&self) -> usize {
        self.doc.col
    }

    pub fn line(&self) -> String {
        self.doc.line(self.doc.row)
    }

    /// Whether keys type into the note: Insert or Replace mode.
    pub fn is_typing(&self) -> bool {
        matches!(self.mode, Mode::Insert | Mode::Replace)
    }

    // ── Undo ──────────────────────────────────────────────────────────────

    fn snap(&self) -> Snap {
        Snap {
            text: self.doc.text.clone(),
            row: self.doc.row,
            col: self.doc.col,
        }
    }

    /// Before a key: a change it starts is remembered from here.
    pub fn begin(&mut self) {
        if self.group.is_none() && self.idle() {
            self.group = Some(self.snap());
        }
    }

    /// After a key: back in Normal mode with nothing pending, what changed
    /// since `begin` is one step to undo. Gives the text before, when it
    /// changed.
    pub fn end(&mut self) -> Option<Text> {
        if !self.idle() {
            return None;
        }
        let group = self.group.take()?;
        if group.text == self.doc.text {
            return None;
        }
        let before = group.text.clone();
        self.undo.push(group);
        self.redo.clear();
        Some(before)
    }

    /// A step to undo starts here whatever the mode, for a paste that comes
    /// in Visual mode.
    pub fn begin_any(&mut self) {
        if self.group.is_none() {
            self.group = Some(self.snap());
        }
    }

    /// A change made outside a key (a menu, a click, a file changed from
    /// outside), as one step to undo, or part of the key's when one is
    /// under way.
    pub fn change(&mut self, edit: impl FnOnce(&mut Doc)) -> bool {
        let before = self.snap();
        edit(&mut self.doc);
        if before.text == self.doc.text {
            return false;
        }
        if self.group.is_none() {
            self.undo.push(before);
        }
        self.redo.clear();
        true
    }

    pub fn undo(&mut self) -> bool {
        // Undoing isn't a change of its own.
        self.group = None;
        let Some(snap) = self.undo.pop() else {
            self.say("Already at oldest change");
            return false;
        };
        let now = self.snap();
        self.redo.push(Snap {
            row: snap.row,
            col: snap.col,
            ..now
        });
        self.doc.text = snap.text;
        self.doc.row = snap.row.min(self.doc.text.len() - 1);
        self.doc.col = snap.col;
        self.clamp();
        true
    }

    pub fn redo(&mut self) -> bool {
        self.group = None;
        let Some(snap) = self.redo.pop() else {
            self.say("Already at newest change");
            return false;
        };
        self.undo.push(self.snap());
        self.doc.text = snap.text;
        self.doc.row = snap.row.min(self.doc.text.len() - 1);
        self.doc.col = snap.col;
        self.clamp();
        true
    }

    // ── The cursor ────────────────────────────────────────────────────────

    /// The cursor kept on a character: in Normal mode on the last at most,
    /// in Insert and Replace mode past it at most.
    pub fn clamp(&mut self) {
        self.doc.row = self.doc.row.min(self.doc.text.len().saturating_sub(1));
        let line = self.line();
        let mut col = self.doc.col.min(line.len());
        while !line.is_char_boundary(col) {
            col -= 1;
        }
        if !self.is_typing() && col == line.len() && col > 0 {
            col = line[..col].char_indices().last().map_or(0, |(i, _)| i);
        }
        self.doc.col = col;
    }

    fn set(&mut self, row: usize, col: usize) {
        self.doc.row = row;
        self.doc.col = col;
        self.clamp();
    }

    /// The byte of the character `ci` of a line, or the line's end.
    fn byte_of(line: &str, ci: usize) -> usize {
        line.char_indices().nth(ci).map_or(line.len(), |(i, _)| i)
    }

    fn char_index(line: &str, col: usize) -> usize {
        line[..col.min(line.len())].chars().count()
    }

    /// The rows shown, one standing for each closed fold.
    fn shown_rows(&self) -> Vec<usize> {
        let kinds = render::kinds(&self.doc.text.lines());
        self.folds
            .visible(&kinds)
            .into_iter()
            .map(|(r, _)| r)
            .collect()
    }

    // ── Motions ───────────────────────────────────────────────────────────

    fn next_pos(&self, (r, i): (usize, usize)) -> Option<(usize, usize)> {
        let len = self.doc.line(r).chars().count();
        if i + 1 < len {
            Some((r, i + 1))
        } else if r + 1 < self.doc.text.len() {
            Some((r + 1, 0))
        } else {
            None
        }
    }

    fn char_at(&self, (r, i): (usize, usize)) -> Option<char> {
        self.doc.line(r).chars().nth(i)
    }

    /// `w`: the start of the next word, an empty line counting as one.
    fn word_fwd(&self, (r, i): (usize, usize), big: bool) -> (usize, usize) {
        let line: Vec<char> = self.doc.line(r).chars().collect();
        let (mut r, mut i) = (r, i);
        if let Some(&c) = line.get(i) {
            let k = class(c, big);
            if k != 0 {
                while i < line.len() && class(line[i], big) == k {
                    i += 1;
                }
            }
        }
        let mut line = line;
        loop {
            if i >= line.len() {
                if r + 1 >= self.doc.text.len() {
                    return (r, line.len().saturating_sub(1));
                }
                r += 1;
                line = self.doc.line(r).chars().collect();
                i = 0;
                if line.is_empty() {
                    return (r, 0);
                }
                continue;
            }
            if class(line[i], big) == 0 {
                i += 1;
                continue;
            }
            return (r, i);
        }
    }

    /// `b`: the start of this word, or of the one before.
    fn word_back(&self, (r, i): (usize, usize), big: bool) -> (usize, usize) {
        let (mut r, mut i) = (r, i);
        let mut line: Vec<char> = self.doc.line(r).chars().collect();
        loop {
            if i == 0 {
                if r == 0 {
                    return (0, 0);
                }
                r -= 1;
                line = self.doc.line(r).chars().collect();
                if line.is_empty() {
                    return (r, 0);
                }
                i = line.len();
            }
            i -= 1;
            if class(line[i], big) != 0 {
                break;
            }
        }
        let k = class(line[i], big);
        while i > 0 && class(line[i - 1], big) == k {
            i -= 1;
        }
        (r, i)
    }

    /// `e`: the end of this word, or of the next.
    fn word_end(&self, pos: (usize, usize), big: bool) -> (usize, usize) {
        let Some(mut p) = self.next_pos(pos) else {
            return pos;
        };
        while self.char_at(p).is_none_or(|c| class(c, big) == 0) {
            match self.next_pos(p) {
                Some(n) => p = n,
                None => return p,
            }
        }
        let line: Vec<char> = self.doc.line(p.0).chars().collect();
        let k = class(line[p.1], big);
        let mut i = p.1;
        while i + 1 < line.len() && class(line[i + 1], big) == k {
            i += 1;
        }
        (p.0, i)
    }

    /// `ge`: the end of the word before.
    fn word_end_back(&self, (r, i): (usize, usize), big: bool) -> (usize, usize) {
        let (mut r, mut i) = (r, i);
        let mut line: Vec<char> = self.doc.line(r).chars().collect();
        let k = line.get(i).map(|c| class(*c, big)).unwrap_or(0);
        // Out of this word, then back over the blanks.
        while i > 0 && k != 0 && class(line[i - 1], big) == k {
            i -= 1;
        }
        loop {
            if i == 0 {
                if r == 0 {
                    return (0, 0);
                }
                r -= 1;
                line = self.doc.line(r).chars().collect();
                if line.is_empty() {
                    return (r, 0);
                }
                i = line.len();
            }
            i -= 1;
            if class(line[i], big) != 0 {
                return (r, i);
            }
        }
    }

    fn find_char(
        &self,
        c: char,
        back: bool,
        till: bool,
        count: usize,
        repeat: bool,
    ) -> Option<usize> {
        let line: Vec<char> = self.line().chars().collect();
        let mut i = Self::char_index(&self.line(), self.doc.col);
        for _ in 0..count {
            let from = if till && repeat {
                if back { i.checked_sub(1)? } else { i + 1 }
            } else {
                i
            };
            let found = if back {
                (0..from).rev().find(|&j| line[j] == c)?
            } else {
                (from + 1..line.len()).find(|&j| line[j] == c)?
            };
            i = found;
        }
        let i = if till {
            if back { i + 1 } else { i - 1 }
        } else {
            i
        };
        Some(Self::byte_of(&self.line(), i))
    }

    /// Where a motion goes, `count` times.
    fn target(&mut self, m: Motion, count: usize, counted: bool) -> Option<(usize, usize)> {
        let (row, col) = (self.doc.row, self.doc.col);
        let line = self.line();
        let ci = Self::char_index(&line, col);
        let last_row = self.doc.text.len() - 1;
        let pos = |me: &Self, r: usize, i: usize| (r, Self::byte_of(&me.doc.line(r), i));
        Some(match m {
            Motion::Left => (row, Self::byte_of(&line, ci.saturating_sub(count))),
            Motion::Right => {
                let len = line.chars().count();
                let max = if self.is_typing() {
                    len
                } else {
                    len.saturating_sub(1)
                };
                (row, Self::byte_of(&line, (ci + count).min(max.max(ci))))
            }
            Motion::Down | Motion::Up => {
                let rows = self.shown_rows();
                let at = rows.iter().position(|&r| r >= row).unwrap_or(0);
                let to = if m == Motion::Down {
                    (at + count).min(rows.len() - 1)
                } else {
                    at.saturating_sub(count)
                };
                let r = rows[to];
                let want = self.want.unwrap_or(ci);
                self.want = Some(want);
                let target = self.doc.line(r);
                (r, Self::byte_of(&target, want.min(target.chars().count())))
            }
            Motion::Word { big } => {
                let mut p = (row, ci);
                for _ in 0..count {
                    p = self.word_fwd(p, big);
                }
                pos(self, p.0, p.1)
            }
            Motion::Back { big } => {
                let mut p = (row, ci);
                for _ in 0..count {
                    p = self.word_back(p, big);
                }
                pos(self, p.0, p.1)
            }
            Motion::End { big } => {
                let mut p = (row, ci);
                for _ in 0..count {
                    p = self.word_end(p, big);
                }
                pos(self, p.0, p.1)
            }
            Motion::EndBack { big } => {
                let mut p = (row, ci);
                for _ in 0..count {
                    p = self.word_end_back(p, big);
                }
                pos(self, p.0, p.1)
            }
            Motion::LineStart => (row, 0),
            Motion::FirstNonBlank => (row, first_non_blank(&line)),
            Motion::LineEnd => {
                let r = (row + count - 1).min(last_row);
                let l = self.doc.line(r);
                self.want = Some(usize::MAX);
                (r, l.len())
            }
            Motion::Goto { first } => {
                let r = if counted {
                    count.saturating_sub(1).min(last_row)
                } else if first {
                    0
                } else {
                    last_row
                };
                (r, first_non_blank(&self.doc.line(r)))
            }
            Motion::Find { c, back, till } => {
                self.last_find = Some((c, back, till));
                (row, self.find_char(c, back, till, count, false)?)
            }
            Motion::RepeatFind { reverse } => {
                let (c, back, till) = self.last_find?;
                (row, self.find_char(c, back != reverse, till, count, true)?)
            }
            Motion::ParagraphFwd | Motion::ParagraphBack => {
                let fwd = m == Motion::ParagraphFwd;
                let mut r = row;
                for _ in 0..count {
                    let blank = |me: &Self, r: usize| me.doc.line(r).is_empty();
                    if fwd {
                        while r < last_row && blank(self, r) {
                            r += 1;
                        }
                        while r < last_row && !blank(self, r) {
                            r += 1;
                        }
                    } else {
                        while r > 0 && blank(self, r) {
                            r -= 1;
                        }
                        while r > 0 && !blank(self, r) {
                            r -= 1;
                        }
                    }
                }
                let col = if fwd && r == last_row && !self.doc.line(r).is_empty() {
                    self.doc.line(r).len()
                } else {
                    0
                };
                (r, col)
            }
            Motion::Match => self.matching()?,
            Motion::Screen(which) => {
                let bottom = (self.top + self.height.max(1) - 1).min(last_row);
                let r = match which {
                    b'H' => (self.top + count - 1).min(bottom),
                    b'L' => bottom.saturating_sub(count - 1).max(self.top),
                    _ => (self.top + bottom) / 2,
                };
                (r, first_non_blank(&self.doc.line(r)))
            }
            Motion::Next { reverse } => self.search_next(reverse, count)?,
            Motion::NextLine => {
                let r = (row + count).min(last_row);
                (r, first_non_blank(&self.doc.line(r)))
            }
            Motion::PrevLine => {
                let r = row.saturating_sub(count);
                (r, first_non_blank(&self.doc.line(r)))
            }
            Motion::CurrentLine => {
                let r = (row + count - 1).min(last_row);
                (r, first_non_blank(&self.doc.line(r)))
            }
        })
    }

    /// `%`: the bracket matching the one under or after the cursor.
    fn matching(&self) -> Option<(usize, usize)> {
        let line = self.line();
        let (at, open) = line[self.doc.col..]
            .char_indices()
            .find(|(_, c)| "()[]{}".contains(*c))
            .map(|(i, c)| (i + self.doc.col, c))?;
        let (pair, fwd) = match open {
            '(' => (')', true),
            '[' => (']', true),
            '{' => ('}', true),
            ')' => ('(', false),
            ']' => ('[', false),
            _ => ('{', false),
        };
        let mut depth = 0;
        let rows: Vec<usize> = if fwd {
            (self.doc.row..self.doc.text.len()).collect()
        } else {
            (0..=self.doc.row).rev().collect()
        };
        for r in rows {
            let l = self.doc.line(r);
            let mut cs: Vec<(usize, char)> = chars_of(&l);
            if fwd {
                cs.retain(|(i, _)| r != self.doc.row || *i >= at);
            } else {
                cs.retain(|(i, _)| r != self.doc.row || *i <= at);
                cs.reverse();
            }
            for (i, c) in cs {
                if c == open {
                    depth += 1;
                } else if c == pair {
                    depth -= 1;
                    if depth == 0 {
                        return Some((r, i));
                    }
                }
            }
        }
        None
    }

    // ── Search ────────────────────────────────────────────────────────────

    fn pattern(text: &str) -> Option<Regex> {
        let smart = if text.chars().any(char::is_uppercase) {
            ""
        } else {
            "(?i)"
        };
        Regex::new(&format!("{smart}{text}"))
            .or_else(|_| Regex::new(&format!("{smart}{}", regex::escape(text))))
            .ok()
    }

    /// Searches for `text`, as `/` and `?` do, and goes to the match.
    pub fn search_for(&mut self, text: &str, forward: bool) {
        if text.is_empty() {
            if self.search.is_none() {
                self.say("E35: No previous regular expression");
                return;
            }
            if let Some(s) = &mut self.search {
                s.1 = forward;
            }
        } else {
            self.search = Some((text.to_string(), forward));
        }
        if let Some(p) = self.search_next(false, 1) {
            self.want = None;
            self.set(p.0, p.1);
        }
    }

    fn search_next(&mut self, reverse: bool, count: usize) -> Option<(usize, usize)> {
        let (text, forward) = self.search.clone()?;
        let Some(re) = Self::pattern(&text) else {
            self.say(format!("E486: Pattern not found: {text}"));
            return None;
        };
        let forward = forward != reverse;
        let lines = self.doc.text.lines();
        let mut matches: Vec<(usize, usize)> = Vec::new();
        for (r, l) in lines.iter().enumerate() {
            matches.extend(re.find_iter(l).map(|m| (r, m.start())));
        }
        if matches.is_empty() {
            self.say(format!("E486: Pattern not found: {text}"));
            return None;
        }
        let mut at = (self.doc.row, self.doc.col);
        for _ in 0..count {
            at = if forward {
                matches
                    .iter()
                    .copied()
                    .find(|&m| m > at)
                    .unwrap_or(matches[0])
            } else {
                matches
                    .iter()
                    .rev()
                    .copied()
                    .find(|&m| m < at)
                    .unwrap_or(*matches.last().unwrap_or(&at))
            };
        }
        Some(at)
    }

    /// `*` and `#`: the word under the cursor, searched for whole.
    fn star(&mut self, forward: bool) {
        let line = self.line();
        let (s, e) = super::format::word_at(&line, self.doc.col);
        if s == e {
            self.say("E348: No string under cursor");
            return;
        }
        self.search = Some((format!(r"\b{}\b", regex::escape(&line[s..e])), forward));
        if let Some(p) = self.search_next(false, 1) {
            self.set(p.0, p.1);
        }
    }

    // ── Changing the text ─────────────────────────────────────────────────

    /// Replaces from one position to another (the end exclusive; a row past
    /// the last is the text's end) with `with`; gives where what was put in
    /// ends.
    fn replace(&mut self, from: (usize, usize), to: (usize, usize), with: &str) -> (usize, usize) {
        let len = self.doc.text.len();
        let (r1, c1) = from;
        let (r2, c2) = if to.0 >= len {
            (len - 1, self.doc.line(len - 1).len())
        } else {
            to
        };
        let first = self.doc.line(r1);
        let last = self.doc.line(r2);
        let prefix = &first[..c1.min(first.len())];
        let suffix = &last[c2.min(last.len())..];
        let joined = format!("{prefix}{with}{suffix}");
        let parts: Vec<&str> = joined.split('\n').collect();
        let end_row = r1 + parts.len() - 1;
        let end_col = parts.last().map_or(0, |p| p.len() - suffix.len());
        self.doc.text.set_lines(r1, r2 + 1, &parts);
        (end_row, end_col)
    }

    /// The text from one position to another, the end exclusive.
    fn text_between(&self, from: (usize, usize), to: (usize, usize)) -> String {
        let len = self.doc.text.len();
        let to = if to.0 >= len {
            (len - 1, self.doc.line(len - 1).len())
        } else {
            to
        };
        if from.0 == to.0 {
            let l = self.doc.line(from.0);
            return l[from.1.min(l.len())..to.1.min(l.len())].to_string();
        }
        let mut out = self.doc.line(from.0)[from.1..].to_string();
        for r in from.0 + 1..to.0 {
            out.push('\n');
            out.push_str(&self.doc.line(r));
        }
        out.push('\n');
        let l = self.doc.line(to.0);
        out.push_str(&l[..to.1.min(l.len())]);
        out
    }

    fn keep(&mut self, text: String, linewise: bool) {
        if self.clipboard {
            self.out.push(Effect::Copy(if linewise {
                format!("{text}\n")
            } else {
                text.clone()
            }));
        }
        self.register = Register {
            text,
            linewise,
            block: false,
        };
    }

    /// A block's rows kept, a line each, as a block to put back as one.
    fn keep_block(&mut self, text: String) {
        if self.clipboard {
            self.out.push(Effect::Copy(text.clone()));
        }
        self.register = Register {
            text,
            linewise: false,
            block: true,
        };
    }

    /// The register to put: the system clipboard's text when it holds
    /// something else than what was kept, whole lines when it ends with a
    /// newline.
    fn to_put(&self) -> Register {
        if self.clipboard
            && let Some(text) = crate::platform::paste()
        {
            let kept = if self.register.linewise {
                format!("{}\n", self.register.text)
            } else {
                self.register.text.clone()
            };
            if !text.is_empty() && text != kept && text.trim_end_matches('\n') != self.register.text
            {
                return match text.strip_suffix('\n') {
                    Some(lines) => Register {
                        text: lines.to_string(),
                        linewise: true,
                        block: false,
                    },
                    None => Register {
                        text,
                        linewise: false,
                        block: false,
                    },
                };
            }
        }
        self.register.clone()
    }

    fn put(&mut self, before: bool, count: usize) {
        let reg = self.to_put();
        if reg.text.is_empty() && !reg.linewise {
            return;
        }
        let (row, col) = (self.doc.row, self.doc.col);
        if reg.block {
            let line = self.line();
            let at = Self::char_index(&line, col) + usize::from(!before && !line.is_empty());
            return self.put_block(&reg.text, row, at, count);
        }
        if reg.linewise {
            let lines: Vec<String> = std::iter::repeat_n(reg.text.as_str(), count)
                .flat_map(|t| t.split('\n'))
                .map(String::from)
                .collect();
            let at = if before { row } else { row + 1 };
            self.doc.text.set_lines(at, at, &lines);
            let fnb = first_non_blank(&self.doc.line(at));
            self.set(at, fnb);
            return;
        }
        let text = reg.text.repeat(count);
        let line = self.line();
        let at = if before || line.is_empty() {
            col
        } else {
            line[col..]
                .chars()
                .next()
                .map_or(col, |c| col + c.len_utf8())
        };
        let end = self.replace((row, at), (row, at), &text);
        if text.contains('\n') {
            self.set(row, at);
        } else {
            let l = self.doc.line(end.0);
            let last = l[..end.1].char_indices().last().map_or(0, |(i, _)| i);
            self.set(end.0, last);
        }
    }

    /// A block put as a column: each of its lines into a row from `row`
    /// down at character `at`, a row too short padded with spaces and
    /// rows added past the text's end, each piece padded to the block's
    /// width where text follows it, as Vim's blockwise put; the cursor on
    /// its top-left.
    fn put_block(&mut self, text: &str, row: usize, at: usize, count: usize) {
        let pieces: Vec<&str> = text.split('\n').collect();
        let width = pieces.iter().map(|p| p.chars().count()).max().unwrap_or(0);
        for (k, piece) in pieces.iter().enumerate() {
            // Each copy but the last padded to the block's width, so a
            // count's copies stay columns on a short row too.
            let short = " ".repeat(width - piece.chars().count());
            let piece = format!("{piece}{short}").repeat(count - 1) + piece;
            let r = row + k;
            if r >= self.doc.text.len() {
                let len = self.doc.text.len();
                self.doc.text.set_lines(len, len, &[""]);
            }
            let mut line = self.doc.line(r);
            let n = line.chars().count();
            if n < at {
                line.push_str(&" ".repeat(at - n));
            }
            let b = Self::byte_of(&line, at);
            let fill = if b < line.len() { short } else { String::new() };
            let new = format!("{}{piece}{fill}{}", &line[..b], &line[b..]);
            self.doc.set_line(r, &new);
        }
        let b = Self::byte_of(&self.doc.line(row), at);
        self.set(row, b);
    }

    /// A line indented or outdented by `width` spaces; gives the bytes
    /// added (taken away as a negative).
    fn indent_line(&mut self, row: usize, out: bool, width: usize) -> isize {
        let line = self.doc.line(row);
        if out {
            let removed = width.min(render::indent_of(&line));
            self.doc.set_line(row, &line[removed..]);
            -(removed as isize)
        } else {
            self.doc
                .set_line(row, &format!("{}{line}", " ".repeat(width)));
            width as isize
        }
    }

    fn indent_rows(&mut self, from: usize, to: usize, out: bool, count: usize) {
        let width = self.doc.opts.shiftwidth * count;
        for r in from..=to {
            if !self.doc.line(r).is_empty() {
                self.indent_line(r, out, width);
            }
        }
        let fnb = first_non_blank(&self.doc.line(from));
        self.set(from, fnb);
    }

    fn case(&mut self, range: Range, op: Op) {
        let (from, to) = match range {
            Range::Chars(a, b) => (a, b),
            Range::Lines(a, b) => ((a, 0), (b, self.doc.line(b).len())),
            Range::Block((a, b), block) => {
                for r in a..=b {
                    if let Some((s, e)) = block.span(&self.doc.line(r)) {
                        self.case(Range::Chars((r, s), (r, e)), op);
                    }
                }
                return self.block_corner(a, block);
            }
        };
        let text = self.text_between(from, to);
        let changed: String = text
            .chars()
            .map(|c| match op {
                Op::Lower => c.to_lowercase().next().unwrap_or(c),
                Op::Upper => c.to_uppercase().next().unwrap_or(c),
                _ if c.is_uppercase() => c.to_lowercase().next().unwrap_or(c),
                _ => c.to_uppercase().next().unwrap_or(c),
            })
            .collect();
        if changed != text {
            self.replace(from, to, &changed);
        }
        self.set(from.0, from.1);
    }

    /// The cursor on a block's top-left, as an operator on one leaves it.
    fn block_corner(&mut self, row: usize, block: Block) {
        let b = Self::byte_of(&self.doc.line(row), block.cols.0);
        self.set(row, b);
    }

    /// Each row's text inside a block, a line each.
    fn block_text(&self, (a, b): (usize, usize), block: Block) -> String {
        (a..=b)
            .map(|r| {
                let line = self.doc.line(r);
                block
                    .span(&line)
                    .map_or(String::new(), |(s, e)| line[s..e].to_string())
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn apply_block(&mut self, op: Op, (a, b): (usize, usize), block: Block) {
        match op {
            Op::Toggle | Op::Lower | Op::Upper => self.case(Range::Block((a, b), block), op),
            Op::Yank => {
                let text = self.block_text((a, b), block);
                self.keep_block(text);
                self.block_corner(a, block);
            }
            Op::Delete | Op::Change => {
                let text = self.block_text((a, b), block);
                self.keep_block(text);
                let skip = self.rows_short_of((a, b), block.cols.0);
                for r in a..=b {
                    if let Some((s, e)) = block.span(&self.doc.line(r)) {
                        self.replace((r, s), (r, e), "");
                    }
                }
                if op == Op::Delete {
                    return self.block_corner(a, block);
                }
                self.start_block_insert((a, b), block.cols.0, false, skip);
            }
            Op::Indent | Op::Outdent => {
                // Shifted at the block's left column, as Vim shifts a block.
                let width = self.doc.opts.shiftwidth;
                for r in a..=b {
                    let line = self.doc.line(r);
                    let Some((s, _)) = block.span(&line) else {
                        continue;
                    };
                    let new = if op == Op::Indent {
                        format!("{}{}{}", &line[..s], " ".repeat(width), &line[s..])
                    } else {
                        let blanks = line[s..].len() - line[s..].trim_start_matches(' ').len();
                        format!("{}{}", &line[..s], &line[s + blanks.min(width)..])
                    };
                    self.doc.set_line(r, &new);
                }
                self.block_corner(a, block);
            }
        }
    }

    /// The rows that don't reach into a block whose left column is `col`.
    fn rows_short_of(&self, (a, b): (usize, usize), col: usize) -> Vec<usize> {
        (a..=b)
            .filter(|&r| self.doc.line(r).chars().count() <= col)
            .collect()
    }

    /// Insert mode on a block's first row that reaches into it, at its
    /// column (padded with spaces to reach it, for `A`), or the line's end
    /// with `at_end`; what is typed there goes into every row on Esc.
    fn start_block_insert(
        &mut self,
        rows: (usize, usize),
        col: usize,
        at_end: bool,
        skip: Vec<usize>,
    ) {
        let Some(row) = (rows.0..=rows.1).find(|r| !skip.contains(r)) else {
            // No row reaches into the block: nothing to type into.
            self.mode = Mode::Normal;
            return self.set(rows.0, self.doc.col);
        };
        let mut line = self.doc.line(row);
        let n = line.chars().count();
        if !at_end && n < col {
            line.push_str(&" ".repeat(col - n));
            self.doc.set_line(row, &line);
        }
        let from = if at_end {
            line.len()
        } else {
            Self::byte_of(&line, col)
        };
        self.block_insert = Some(BlockInsert {
            rows,
            row,
            at: (row, from),
            col,
            at_end,
            skip,
            from,
        });
        self.mode = Mode::Insert;
        self.doc.row = row;
        self.doc.col = from;
    }

    /// Esc after typing into a block: what was typed on the row typed on
    /// put into each of the others, unless it broke the line or went back past
    /// where it started, as Vim drops it then.
    fn finish_block_insert(&mut self, bi: BlockInsert) {
        let (row, col) = (self.doc.row, self.doc.col);
        if row != bi.row || col < bi.from {
            return;
        }
        let typed = self.line()[bi.from..col].to_string();
        if typed.is_empty() {
            return;
        }
        for r in (bi.rows.0..=bi.rows.1).filter(|&r| r != bi.row) {
            let mut line = self.doc.line(r);
            let n = line.chars().count();
            let at = if bi.at_end {
                line.len()
            } else if bi.skip.contains(&r) {
                continue;
            } else if n < bi.col {
                line.push_str(&" ".repeat(bi.col - n));
                line.len()
            } else {
                Self::byte_of(&line, bi.col)
            };
            self.doc
                .set_line(r, &format!("{}{typed}{}", &line[..at], &line[at..]));
        }
        self.doc.col = bi.from;
    }

    fn apply(&mut self, op: Op, range: Range, _count: usize) {
        if let Range::Block(rows, block) = range {
            return self.apply_block(op, rows, block);
        }
        match (op, range) {
            (Op::Indent | Op::Outdent, Range::Lines(a, b)) => {
                self.indent_rows(a, b, op == Op::Outdent, 1);
            }
            (Op::Indent | Op::Outdent, Range::Chars(a, b)) => {
                self.indent_rows(a.0, b.0, op == Op::Outdent, 1);
            }
            (Op::Toggle | Op::Lower | Op::Upper, r) => self.case(r, op),
            (Op::Yank, Range::Lines(a, b)) => {
                let text = (a..=b)
                    .map(|r| self.doc.line(r))
                    .collect::<Vec<_>>()
                    .join("\n");
                self.keep(text, true);
                let col = self.doc.col;
                self.set(a.min(self.doc.row), col);
            }
            (Op::Yank, Range::Chars(a, b)) => {
                let text = self.text_between(a, b);
                self.keep(text, false);
                self.set(a.0, a.1);
            }
            (Op::Delete, Range::Lines(a, b)) => {
                let text = (a..=b)
                    .map(|r| self.doc.line(r))
                    .collect::<Vec<_>>()
                    .join("\n");
                self.keep(text, true);
                self.doc.text.set_lines(a, b + 1, &[] as &[&str]);
                let r = a.min(self.doc.text.len() - 1);
                let fnb = first_non_blank(&self.doc.line(r));
                self.set(r, fnb);
            }
            (Op::Delete, Range::Chars(a, b)) => {
                let text = self.text_between(a, b);
                self.keep(text, false);
                self.replace(a, b, "");
                self.set(a.0, a.1);
            }
            (Op::Change, Range::Lines(a, b)) => {
                let text = (a..=b)
                    .map(|r| self.doc.line(r))
                    .collect::<Vec<_>>()
                    .join("\n");
                self.keep(text, true);
                let indent: String = self
                    .doc
                    .line(a)
                    .chars()
                    .take_while(|c| c.is_whitespace())
                    .collect();
                self.doc.text.set_lines(a, b + 1, &[indent.as_str()]);
                self.mode = Mode::Insert;
                self.doc.row = a;
                self.doc.col = indent.len();
            }
            (Op::Change, Range::Chars(a, b)) => {
                let text = self.text_between(a, b);
                self.keep(text, false);
                self.replace(a, b, "");
                self.mode = Mode::Insert;
                self.doc.row = a.0;
                self.doc.col = a.1;
            }
            (_, Range::Block(..)) => {}
        }
    }

    /// The range a motion covers from the cursor, as an operator takes it.
    fn motion_range(&mut self, m: Motion, count: usize, counted: bool, op: Op) -> Option<Range> {
        let start = (self.doc.row, self.doc.col);
        let mut m = m;
        // `cw` on a word changes to its end, as `ce` does.
        if op == Op::Change
            && let Motion::Word { big } = m
            && self
                .char_at((start.0, Self::char_index(&self.line(), start.1)))
                .is_some_and(|c| !c.is_whitespace())
        {
            m = Motion::End { big };
        }
        let target = self.target(m, count, counted)?;
        if m.linewise() {
            let (a, b) = if target.0 < start.0 {
                (target.0, start.0)
            } else {
                (start.0, target.0)
            };
            return Some(Range::Lines(a, b));
        }
        let (mut a, mut b) = if target < start {
            (target, start)
        } else {
            (start, target)
        };
        if m.inclusive() {
            let l = self.doc.line(b.0);
            b.1 = l[b.1.min(l.len())..]
                .chars()
                .next()
                .map_or(l.len(), |c| b.1 + c.len_utf8());
        } else if matches!(m, Motion::Word { .. }) && b.0 > a.0 {
            // `dw` on a line's last word stops at the line's end.
            let fnb = first_non_blank(&self.doc.line(b.0));
            if b.1 <= fnb {
                b = (b.0 - 1, self.doc.line(b.0 - 1).len());
            }
        }
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        Some(Range::Chars(a, b))
    }

    /// A text object around the cursor: a word, a quoted string, a bracketed
    /// one, a paragraph.
    fn object(&self, inner: bool, what: char, count: usize) -> Option<Range> {
        let row = self.doc.row;
        let line = self.line();
        let col = self.doc.col.min(line.len());
        match what {
            'w' | 'W' => {
                let big = what == 'W';
                let cs = chars_of(&line);
                if cs.is_empty() {
                    return None;
                }
                let mut i = Self::char_index(&line, col).min(cs.len() - 1);
                let mut s = i;
                let k = class(cs[i].1, big);
                while s > 0 && class(cs[s - 1].1, big) == k {
                    s -= 1;
                }
                for n in 0..count {
                    if n > 0 {
                        i += 1;
                        if i >= cs.len() {
                            break;
                        }
                    }
                    let k = class(cs[i].1, big);
                    while i + 1 < cs.len() && class(cs[i + 1].1, big) == k {
                        i += 1;
                    }
                }
                let mut e = i + 1;
                if !inner && k == 0 {
                    // On blanks: the blanks and the word after them.
                    if e < cs.len() {
                        let next = class(cs[e].1, big);
                        while e < cs.len() && class(cs[e].1, big) == next {
                            e += 1;
                        }
                    }
                } else if !inner {
                    // The blanks after it, or else those before it.
                    let after = e;
                    while e < cs.len() && cs[e].1.is_whitespace() {
                        e += 1;
                    }
                    if e == after {
                        while s > 0 && cs[s - 1].1.is_whitespace() {
                            s -= 1;
                        }
                    }
                }
                let byte = |i: usize| cs.get(i).map_or(line.len(), |c| c.0);
                Some(Range::Chars((row, byte(s)), (row, byte(e))))
            }
            '"' | '\'' | '`' => {
                let quotes: Vec<usize> = line.match_indices(what).map(|(i, _)| i).collect();
                // The pair around the cursor, else the next on the line.
                let pairs: Vec<(usize, usize)> = quotes
                    .chunks(2)
                    .filter(|p| p.len() == 2)
                    .map(|p| (p[0], p[1]))
                    .collect();
                let (s, e) = pairs
                    .iter()
                    .copied()
                    .find(|&(a, b)| col >= a && col <= b)
                    .or_else(|| pairs.iter().copied().find(|&(a, _)| a > col))?;
                if inner {
                    Some(Range::Chars((row, s + 1), (row, e)))
                } else {
                    Some(Range::Chars((row, s), (row, e + 1)))
                }
            }
            '(' | ')' | 'b' | '[' | ']' | '{' | '}' | 'B' | '<' | '>' => {
                let (open, close) = match what {
                    '(' | ')' | 'b' => ('(', ')'),
                    '[' | ']' => ('[', ']'),
                    '<' | '>' => ('<', '>'),
                    _ => ('{', '}'),
                };
                let text = self.doc.text.lines();
                // Back to the opening bracket around the cursor.
                let mut depth = 0;
                let mut start = None;
                'out: for r in (0..=row).rev() {
                    let cs = chars_of(&text[r]);
                    for &(i, c) in cs.iter().rev() {
                        if r == row && i > col {
                            continue;
                        }
                        if c == close && !(r == row && i == col) {
                            depth += 1;
                        } else if c == open {
                            if depth == 0 {
                                start = Some((r, i));
                                break 'out;
                            }
                            depth -= 1;
                        }
                    }
                }
                let start = start?;
                let mut depth = 0;
                let mut end = None;
                'out: for (r, l) in text.iter().enumerate().skip(start.0) {
                    for (i, c) in l.char_indices() {
                        if r == start.0 && i <= start.1 {
                            continue;
                        }
                        if c == open {
                            depth += 1;
                        } else if c == close {
                            if depth == 0 {
                                end = Some((r, i));
                                break 'out;
                            }
                            depth -= 1;
                        }
                    }
                }
                let end = end?;
                if inner {
                    Some(Range::Chars((start.0, start.1 + 1), end))
                } else {
                    Some(Range::Chars(start, (end.0, end.1 + 1)))
                }
            }
            'p' => {
                let text = self.doc.text.lines();
                let blank = |r: usize| text[r].trim().is_empty();
                let kind = blank(row);
                let (mut a, mut b) = (row, row);
                while a > 0 && blank(a - 1) == kind {
                    a -= 1;
                }
                while b + 1 < text.len() && blank(b + 1) == kind {
                    b += 1;
                }
                if !inner {
                    while b + 1 < text.len() && blank(b + 1) != kind {
                        b += 1;
                    }
                }
                Some(Range::Lines(a, b))
            }
            _ => None,
        }
    }

    // ── Keys ──────────────────────────────────────────────────────────────

    /// A key, in whatever mode the editor is in.
    pub fn key(&mut self, key: Key) {
        let one_command = self.mode == Mode::Normal && self.one_command();
        // `"` and a register's name, which `normal_key` drops: the
        // command comes after it.
        let naming_a_register = self.keys.len() == 1 && typed(&self.keys[0]) == Some('"');
        match self.mode {
            Mode::Insert => self.insert_key(key),
            Mode::Replace => self.replace_key(key),
            Mode::Visual { .. } => self.visual_key(key),
            Mode::Normal => self.normal_key(key),
        }
        if let Some(how) = self.doc.insert.take() {
            self.mode = Mode::Insert;
            if how == Insert::End {
                self.doc.col = self.line().len();
            }
        }
        self.clamp();
        // A command typed at a prompt (`:`, `/`) runs once the prompt is
        // done, which says so (`prompt_done`).
        let prompting = self.out.iter().any(|e| matches!(e, Effect::Prompt(_)));
        if one_command && self.keys.is_empty() && !naming_a_register && !prompting {
            self.back_to_insert();
        }
    }

    /// The prompt a command asked for is done (run or canceled): Ctrl-O's
    /// command with it, so typing goes on.
    pub fn prompt_done(&mut self) {
        if self.mode == Mode::Normal && self.keys.is_empty() {
            self.back_to_insert();
        }
    }

    /// After Ctrl-O's command (or what was no command): Insert mode
    /// again, unless the command left Normal mode itself, and the cursor
    /// back past the line's end when the command left it where it was.
    fn back_to_insert(&mut self) {
        let Some(Waiting::OneCommand { at }) = self.waiting.take() else {
            return;
        };
        if self.mode != Mode::Normal {
            return;
        }
        let now = (self.doc.row, self.doc.col);
        let unchanged = self.group.as_ref().is_some_and(|g| g.text == self.doc.text);
        (self.doc.row, self.doc.col) = at;
        self.clamp();
        let unmoved = unchanged && (self.doc.row, self.doc.col) == now;
        self.mode = Mode::Insert;
        (self.doc.row, self.doc.col) = if unmoved { at } else { now };
        self.clamp();
        // What is typed after the command is a step of its own.
        self.begin_any();
    }

    /// The count typed so far, when the keys pending are only that (as
    /// `parse` reads one: digits not starting with `0`), taken off them;
    /// else nothing, the keys left as they are.
    pub fn take_count(&mut self) -> Option<usize> {
        let (n, len) = leading_count(&self.keys);
        if len == 0 || len != self.keys.len() {
            return None;
        }
        self.keys.clear();
        Some(n)
    }

    /// Enters Insert mode at a byte of the cursor's line; what is typed
    /// until it is left is one step to undo.
    pub fn insert_at(&mut self, col: usize) {
        self.begin_any();
        self.mode = Mode::Insert;
        self.doc.col = col;
        self.clamp();
    }

    fn parse(&self) -> Parse {
        let keys = &self.keys;
        let mut i = 0;
        let count_at = |i: &mut usize| -> Option<usize> {
            let mut n: Option<usize> = None;
            while let Some(c) = keys.get(*i).and_then(typed) {
                let Some(d) = c.to_digit(10) else { break };
                if d == 0 && n.is_none() {
                    break;
                }
                n = Some(n.unwrap_or(0).saturating_mul(10).saturating_add(d as usize));
                *i += 1;
            }
            n
        };
        let count1 = count_at(&mut i);
        let Some(key) = keys.get(i) else {
            return Parse::More;
        };
        let count = count1.unwrap_or(1);
        // A motion, or what it is not.
        let motion = |keys: &[Key], i: usize| -> Result<(Motion, usize), bool> {
            let Some(key) = keys.get(i) else {
                return Err(true);
            };
            let m = match key.code {
                KeyCode::Left | KeyCode::Backspace => Motion::Left,
                KeyCode::Right => Motion::Right,
                KeyCode::Down => Motion::Down,
                KeyCode::Up => Motion::Up,
                KeyCode::Home => Motion::LineStart,
                KeyCode::End => Motion::LineEnd,
                KeyCode::Enter => Motion::NextLine,
                _ if ctrl(key, 'n') || ctrl(key, 'j') => Motion::Down,
                _ if ctrl(key, 'p') => Motion::Up,
                _ => {
                    let Some(c) = typed(key) else {
                        return Err(false);
                    };
                    match c {
                        'h' => Motion::Left,
                        'l' | ' ' => Motion::Right,
                        'j' => Motion::Down,
                        'k' => Motion::Up,
                        'w' => Motion::Word { big: false },
                        'W' => Motion::Word { big: true },
                        'b' => Motion::Back { big: false },
                        'B' => Motion::Back { big: true },
                        'e' => Motion::End { big: false },
                        'E' => Motion::End { big: true },
                        '0' => Motion::LineStart,
                        '^' => Motion::FirstNonBlank,
                        '$' => Motion::LineEnd,
                        'G' => Motion::Goto { first: false },
                        ';' => Motion::RepeatFind { reverse: false },
                        ',' => Motion::RepeatFind { reverse: true },
                        '}' => Motion::ParagraphFwd,
                        '{' => Motion::ParagraphBack,
                        '%' => Motion::Match,
                        'H' => Motion::Screen(b'H'),
                        'M' => Motion::Screen(b'M'),
                        'L' => Motion::Screen(b'L'),
                        'n' => Motion::Next { reverse: false },
                        'N' => Motion::Next { reverse: true },
                        '+' => Motion::NextLine,
                        '-' => Motion::PrevLine,
                        '_' => Motion::CurrentLine,
                        'f' | 'F' | 't' | 'T' => {
                            let Some(target) = keys.get(i + 1) else {
                                return Err(true);
                            };
                            let Some(t) = typed(target) else {
                                return Err(false);
                            };
                            return Ok((
                                Motion::Find {
                                    c: t,
                                    back: c == 'F' || c == 'T',
                                    till: c == 't' || c == 'T',
                                },
                                2,
                            ));
                        }
                        'g' => {
                            let Some(next) = keys.get(i + 1) else {
                                return Err(true);
                            };
                            return match typed(next) {
                                Some('g') => Ok((Motion::Goto { first: true }, 2)),
                                Some('e') => Ok((Motion::EndBack { big: false }, 2)),
                                Some('E') => Ok((Motion::EndBack { big: true }, 2)),
                                Some('j') => Ok((Motion::Down, 2)),
                                Some('k') => Ok((Motion::Up, 2)),
                                Some('_') => Ok((Motion::LineEnd, 2)),
                                _ => Err(false),
                            };
                        }
                        _ => return Err(false),
                    }
                }
            };
            Ok((m, 1))
        };
        // An operator, with its own count and what it acts on.
        let op = match typed(key) {
            Some('d') => Some((Op::Delete, 1)),
            Some('c') => Some((Op::Change, 1)),
            Some('y') => Some((Op::Yank, 1)),
            Some('>') => Some((Op::Indent, 1)),
            Some('<') => Some((Op::Outdent, 1)),
            Some('g') => match keys.get(i + 1).and_then(typed) {
                None if keys.get(i + 1).is_none() => return Parse::More,
                Some('~') => Some((Op::Toggle, 2)),
                Some('u') => Some((Op::Lower, 2)),
                Some('U') => Some((Op::Upper, 2)),
                _ => None,
            },
            _ => None,
        };
        if let Some((op, len)) = op {
            let mut j = i + len;
            let count2 = count_at(&mut j);
            let n = count * count2.unwrap_or(1);
            let counted = count1.is_some() || count2.is_some();
            let Some(next) = keys.get(j) else {
                return Parse::More;
            };
            // Doubled: dd, cc, yy, >>, <<, g~~, guu, gUU.
            let same = matches!(
                (op, typed(next)),
                (Op::Delete, Some('d'))
                    | (Op::Change, Some('c'))
                    | (Op::Yank, Some('y'))
                    | (Op::Indent, Some('>'))
                    | (Op::Outdent, Some('<'))
                    | (Op::Toggle, Some('~'))
                    | (Op::Lower, Some('u'))
                    | (Op::Upper, Some('U'))
            );
            if same {
                return Parse::Done(Command::Operate(op, n, Target::Same));
            }
            if let Some(c @ ('i' | 'a')) = typed(next) {
                return match keys.get(j + 1).and_then(typed) {
                    None if keys.get(j + 1).is_none() => Parse::More,
                    Some(w) => Parse::Done(Command::Operate(
                        op,
                        n,
                        Target::Object {
                            inner: c == 'i',
                            what: w,
                        },
                    )),
                    None => Parse::Invalid,
                };
            }
            return match motion(keys, j) {
                Ok((m, _)) => {
                    let _ = counted;
                    Parse::Done(Command::Operate(
                        op,
                        if counted { n } else { 0 },
                        Target::Motion(m),
                    ))
                }
                Err(true) => Parse::More,
                Err(false) => Parse::Invalid,
            };
        }
        match motion(keys, i) {
            Ok((m, _)) => {
                return Parse::Done(Command::Move(m, if count1.is_some() { count } else { 0 }));
            }
            Err(true) => return Parse::More,
            Err(false) => {}
        }
        let Some(c) = typed(key) else {
            // Ctrl keys and the rest, by their code.
            return Parse::Done(Command::Simple('\0', count));
        };
        match c {
            'r' => match keys.get(i + 1) {
                None => Parse::More,
                Some(k) => match typed(k) {
                    Some(t) => Parse::Done(Command::Replace(t, count)),
                    None if k.code == KeyCode::Enter => Parse::Done(Command::Replace('\n', count)),
                    None => Parse::Invalid,
                },
            },
            // Marks, macros and the rest of the two-key commands: the second
            // key read, so it isn't taken for a command of its own.
            'z' | 'Z' | 'g' | 'm' | '`' | '\'' | 'q' | '@' => match keys.get(i + 1).and_then(typed)
            {
                None if keys.get(i + 1).is_none() => Parse::More,
                Some(t) => Parse::Done(Command::Two(c, t, count)),
                None => Parse::Invalid,
            },
            // A register named: only the unnamed one, which is the
            // clipboard's too, is kept, so `normal_key` lets the name go.
            '"' => Parse::More,
            _ => Parse::Done(Command::Simple(c, count)),
        }
    }

    fn normal_key(&mut self, key: Key) {
        if key.code == KeyCode::Esc && self.keys.is_empty() {
            return;
        }
        if key.code == KeyCode::Esc {
            self.keys.clear();
            return;
        }
        // A register named first ("+y, "*p): the system clipboard is the
        // unnamed register here, so the name is dropped.
        if self.keys.len() == 1 && typed(&self.keys[0]) == Some('"') {
            self.keys.clear();
            return;
        }
        // Ctrl-W and a direction: the window beside this one.
        if self.keys.len() == 1 && ctrl(&self.keys[0], 'w') {
            self.keys.clear();
            let dir = match key.code {
                KeyCode::Left => 'h',
                KeyCode::Right => 'l',
                _ => typed(&key)
                    .or_else(|| ctrl(&key, 'w').then_some('w'))
                    .unwrap_or('?'),
            };
            self.out.push(Effect::Window(dir));
            return;
        }
        if self.keys.is_empty() && ctrl(&key, 'w') {
            self.keys.push(key);
            return;
        }
        // Shift with an arrow: a selection in easy mode, a word else.
        let arrow = matches!(
            key.code,
            KeyCode::Left
                | KeyCode::Right
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Home
                | KeyCode::End
        );
        if self.keys.is_empty() && arrow && key.mods.shift && !key.mods.ctrl {
            return self.ctrl_key(key, 1);
        }
        self.keys.push(key);
        match self.parse() {
            Parse::More => {}
            Parse::Invalid => self.keys.clear(),
            Parse::Done(cmd) => {
                let last = *self.keys.last().unwrap_or(&key);
                self.keys.clear();
                self.run(cmd, last);
            }
        }
    }

    fn run(&mut self, cmd: Command, key: Key) {
        match cmd {
            Command::Move(m, count) => {
                let counted = count > 0;
                let count = count.max(1);
                if !matches!(m, Motion::Down | Motion::Up | Motion::LineEnd) {
                    self.want = None;
                }
                if let Some((r, c)) = self.target(m, count, counted) {
                    self.set(r, c);
                }
            }
            Command::Operate(op, n, target) => {
                let counted = n > 0;
                let count = n.max(1);
                let range = match target {
                    Target::Same => {
                        let a = self.doc.row;
                        let b = (a + count - 1).min(self.doc.text.len() - 1);
                        Some(Range::Lines(a, b))
                    }
                    Target::Object { inner, what } => self.object(inner, what, count),
                    Target::Motion(m) => self.motion_range(m, count, counted, op),
                };
                if let Some(range) = range {
                    self.apply(op, range, count);
                }
            }
            Command::Replace(c, count) => {
                let line = self.line();
                let cs = chars_of(&line);
                let i = Self::char_index(&line, self.doc.col);
                if i + count > cs.len() {
                    return;
                }
                let end = cs.get(i + count).map_or(line.len(), |c| c.0);
                let (row, col) = (self.doc.row, self.doc.col);
                if c == '\n' {
                    self.replace((row, col), (row, end), "\n");
                    self.set(row + 1, 0);
                } else {
                    let with: String = std::iter::repeat_n(c, count).collect();
                    self.replace((row, col), (row, end), &with);
                    let last = cs.get(i + count - 1).map_or(col, |c| c.0);
                    self.set(row, last);
                }
            }
            Command::Two(a, b, count) => self.two(a, b, count),
            Command::Simple(c, count) => self.simple(c, count, key),
        }
    }

    fn two(&mut self, a: char, b: char, count: usize) {
        let kinds = render::kinds(&self.doc.text.lines());
        let row = self.doc.row;
        match (a, b) {
            ('Z', 'Z') => self.out.push(Effect::SaveQuit),
            ('Z', 'Q') => self.out.push(Effect::Quit),
            ('z', 'c') => self.folds.close(&kinds, row),
            ('z', 'o') => self.folds.open(&kinds, row),
            ('z', 'a') => {
                self.folds.toggle(&kinds, row);
            }
            ('z', 'M') => self.folds.close_all(&kinds),
            ('z', 'R') => self.folds.open_all(),
            ('z', 'z') => {
                self.top = row.saturating_sub(self.height / 2);
                self.center = true;
            }
            ('z', 't') => self.top = row,
            ('z', 'b') => self.top = row.saturating_sub(self.height.saturating_sub(1)),
            ('g', 'J') => self.join(count, false),
            ('g', 'v') => {
                self.mode = Mode::Visual {
                    line: false,
                    select: false,
                    block: false,
                }
            }
            ('g', 'i') => self.mode = Mode::Insert,
            ('g', 'w') => self.out.push(Effect::WorkOnItem),
            ('g', 'a') => self.out.push(Effect::AddItem),
            _ => {}
        }
        if a == 'z' {
            // A fold closed over the cursor puts it on the fold's heading.
            if let Some((s, _)) = self.folds.closed_at(&kinds, self.doc.row) {
                self.doc.row = s;
            }
        }
    }

    fn join(&mut self, count: usize, spaces: bool) {
        let row = self.doc.row;
        let n = count.max(2) - 1;
        for _ in 0..n {
            if row + 1 >= self.doc.text.len() {
                break;
            }
            let line = self.doc.line(row);
            let next = self.doc.line(row + 1);
            let rest = if spaces {
                next.trim_start()
            } else {
                next.as_str()
            };
            let sep = if !spaces
                || rest.is_empty()
                || line.ends_with(' ')
                || rest.starts_with(')')
                || line.is_empty()
            {
                ""
            } else {
                " "
            };
            let col = line.len();
            self.doc
                .text
                .set_lines(row, row + 2, &[format!("{line}{sep}{rest}")]);
            self.doc.col = col;
        }
    }

    fn simple(&mut self, c: char, count: usize, key: Key) {
        let (row, col) = (self.doc.row, self.doc.col);
        let line = self.line();
        match c {
            '\0' => self.ctrl_key(key, count),
            'i' => self.mode = Mode::Insert,
            'a' => {
                self.mode = Mode::Insert;
                self.doc.col = line[col..]
                    .chars()
                    .next()
                    .map_or(col, |c| col + c.len_utf8());
            }
            'I' => self.insert_at(first_non_blank(&line)),
            'R' => {
                self.begin_any();
                self.over.clear();
                self.typed_over.clear();
                self.over_count = count.max(1);
                self.over_end = (row, col);
                self.mode = Mode::Replace;
            }
            'A' => self.insert_at(line.len()),
            'o' | 'O' => {
                let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
                let at = if c == 'o' { row + 1 } else { row };
                self.doc.text.set_lines(at, at, &[indent.as_str()]);
                self.doc.row = at;
                self.insert_at(indent.len());
            }
            'x' | 'X' => {
                if line.is_empty() {
                    return;
                }
                let i = Self::char_index(&line, col);
                let cs = chars_of(&line);
                let range = if c == 'x' {
                    let end = cs.get(i + count).map_or(line.len(), |c| c.0);
                    Range::Chars((row, col), (row, end))
                } else {
                    let from = cs.get(i.saturating_sub(count)).map_or(0, |c| c.0);
                    Range::Chars((row, from), (row, col))
                };
                self.apply(Op::Delete, range, count);
            }
            's' => {
                let i = Self::char_index(&line, col);
                let end = chars_of(&line).get(i + count).map_or(line.len(), |c| c.0);
                self.apply(Op::Change, Range::Chars((row, col), (row, end)), count);
            }
            'S' => {
                let b = (row + count - 1).min(self.doc.text.len() - 1);
                self.apply(Op::Change, Range::Lines(row, b), count);
            }
            'C' | 'D' => {
                let end = (row + count - 1).min(self.doc.text.len() - 1);
                let to = (end, self.doc.line(end).len());
                let op = if c == 'C' { Op::Change } else { Op::Delete };
                self.apply(op, Range::Chars((row, col), to), count);
            }
            'Y' => {
                let b = (row + count - 1).min(self.doc.text.len() - 1);
                self.apply(Op::Yank, Range::Lines(row, b), count);
            }
            'p' => self.put(false, count),
            'P' => self.put(true, count),
            'J' => self.join(count, true),
            '~' => {
                let i = Self::char_index(&line, col);
                let end = chars_of(&line).get(i + count).map_or(line.len(), |c| c.0);
                self.case(Range::Chars((row, col), (row, end)), Op::Toggle);
                self.doc.col = end;
            }
            'u' => {
                self.undo();
            }
            'v' => {
                self.anchor = (row, col);
                self.mode = Mode::Visual {
                    line: false,
                    select: false,
                    block: false,
                };
            }
            'V' => {
                self.anchor = (row, col);
                self.mode = Mode::Visual {
                    line: true,
                    select: false,
                    block: false,
                };
            }
            '*' => self.star(true),
            '#' => self.star(false),
            '/' | '?' | ':' => self.out.push(Effect::Prompt(c)),
            _ => {}
        }
    }

    fn ctrl_key(&mut self, key: Key, count: usize) {
        let half = (self.height / 2).max(1);
        let last = self.doc.text.len() - 1;
        if ctrl(&key, 'r') {
            for _ in 0..count {
                self.redo();
            }
        } else if ctrl(&key, 'd') || key.code == KeyCode::PageDown {
            let by = if key.code == KeyCode::PageDown {
                self.height.max(2) - 2
            } else {
                half
            };
            self.top = (self.top + by).min(last);
            let r = (self.doc.row + by).min(last);
            let fnb = first_non_blank(&self.doc.line(r));
            self.set(r, fnb);
        } else if ctrl(&key, 'u') || key.code == KeyCode::PageUp {
            let by = if key.code == KeyCode::PageUp {
                self.height.max(2) - 2
            } else {
                half
            };
            self.top = self.top.saturating_sub(by);
            let r = self.doc.row.saturating_sub(by);
            let fnb = first_non_blank(&self.doc.line(r));
            self.set(r, fnb);
        } else if ctrl(&key, 'a') || ctrl(&key, 'x') {
            let by = i64::try_from(count).unwrap_or(i64::MAX);
            self.add_to_number(if ctrl(&key, 'a') { by } else { -by });
        } else if ctrl(&key, 'v') {
            self.anchor = (self.doc.row, self.doc.col);
            self.anchor_col = Self::char_index(&self.line(), self.doc.col);
            self.want = None;
            self.mode = Mode::Visual {
                line: false,
                select: false,
                block: true,
            };
        } else if ctrl(&key, 'e') {
            self.top = (self.top + count).min(last);
            if self.doc.row < self.top {
                self.doc.row = self.top;
            }
        } else if ctrl(&key, 'y') {
            self.top = self.top.saturating_sub(count);
            if self.doc.row >= self.top + self.height {
                self.doc.row = self.top + self.height.saturating_sub(1);
            }
        } else if self.easy && key.mods.shift && !key.mods.ctrl {
            self.start_select(key);
        } else if key.mods.shift && matches!(key.code, KeyCode::Right | KeyCode::Left) {
            let m = if key.code == KeyCode::Right {
                Motion::Word { big: false }
            } else {
                Motion::Back { big: false }
            };
            if let Some(p) = self.target(m, count, false) {
                self.set(p.0, p.1);
            }
        }
    }

    /// Ctrl-A and Ctrl-X: the number under or after the cursor changed by
    /// `by`, the cursor left on its last digit, as Vim leaves it; a line
    /// with no number is left alone.
    fn add_to_number(&mut self, by: i64) {
        let row = self.doc.row;
        let line = self.line();
        let Some((start, end)) = number_at(&line, self.doc.col) else {
            return;
        };
        let Some(result) = add_to_number(&line[start..end], by) else {
            return;
        };
        let (_, col) = self.replace((row, start), (row, end), &result);
        self.want = None;
        self.set(row, col.saturating_sub(1));
    }

    /// Easy mode's Shift with an arrow: a selection starts, as `keymodel`'s
    /// `startsel` starts one.
    fn start_select(&mut self, key: Key) {
        let m = match key.code {
            KeyCode::Left => Motion::Left,
            KeyCode::Right => Motion::Right,
            KeyCode::Up => Motion::Up,
            KeyCode::Down => Motion::Down,
            KeyCode::Home => Motion::LineStart,
            KeyCode::End => Motion::LineEnd,
            _ => return,
        };
        self.anchor = (self.doc.row, self.doc.col);
        self.mode = Mode::Visual {
            line: false,
            select: true,
            block: false,
        };
        if let Some(p) = self.target(m, 1, false) {
            self.doc.row = p.0;
            self.doc.col = p.1;
        }
    }

    // ── Insert mode ───────────────────────────────────────────────────────

    /// Text typed or pasted at the cursor.
    pub fn type_text(&mut self, text: &str) {
        let (row, col) = (self.doc.row, self.doc.col);
        let end = self.replace((row, col), (row, col), text);
        self.doc.row = end.0;
        self.doc.col = end.1;
    }

    /// Ctrl-R and a register's name: what it holds typed in. `"`, `0`,
    /// `+` and `*` are the one register, which is the clipboard too, and
    /// `/` the last search; whole lines come with their newline.
    fn type_register(&mut self, key: Key) {
        let Some(name) = typed(&key) else {
            return;
        };
        let text = match name {
            '"' | '0' | '+' | '*' => {
                let reg = self.to_put();
                if reg.linewise {
                    format!("{}\n", reg.text)
                } else {
                    reg.text
                }
            }
            '/' => self
                .search
                .as_ref()
                .map(|(s, _)| s.clone())
                .unwrap_or_default(),
            _ => {
                self.say(format!("E353: Nothing in register {name}"));
                return;
            }
        };
        self.type_text(&text);
    }

    fn insert_key(&mut self, key: Key) {
        self.forget_want(key);
        self.typing_into_block(|me| me.insert_key_here(key));
    }

    /// While typing, any key but Up and Down moves the column j and k
    /// would keep, as Vim's do.
    pub fn forget_want(&mut self, key: Key) {
        if !matches!(key.code, KeyCode::Up | KeyCode::Down) {
            self.want = None;
        }
    }

    /// Text the note types in Insert mode (a paste, a link completed, a key
    /// typed literally), typed into every row of a block as typed keys are.
    pub fn paste_text(&mut self, text: &str) {
        self.typing_into_block(|me| me.type_text(text));
    }

    /// An edit while typing, with typing into a block kept only while the
    /// cursor stays on the row typed on, from where the last key left it
    /// (not a click away) and never back past where the typing started.
    fn typing_into_block(&mut self, edit: impl FnOnce(&mut Self)) {
        let here = (self.doc.row, self.doc.col);
        if self.block_insert.as_ref().is_some_and(|bi| bi.at != here) {
            self.block_insert = None;
        }
        let rows = self.doc.text.len();
        edit(self);
        let here = (self.doc.row, self.doc.col);
        // Lines joined or broken move the block's rows under it.
        let reshaped = self.mode != Mode::Insert || self.doc.text.len() != rows;
        if let Some(bi) = &mut self.block_insert {
            if reshaped || here.0 != bi.row || here.1 < bi.from {
                self.block_insert = None;
            } else {
                bi.at = here;
            }
        }
    }

    fn insert_key_here(&mut self, key: Key) {
        // A change made outside a key (an Ex command, a file changed from
        // outside) may have left the cursor past its line.
        self.clamp();
        let (row, col) = (self.doc.row, self.doc.col);
        let line = self.line();
        if self.waiting.take() == Some(Waiting::Register) {
            return self.type_register(key);
        }
        if self.easy
            && key.mods.shift
            && !key.mods.ctrl
            && !matches!(key.code, KeyCode::Char(_) | KeyCode::BackTab)
        {
            return self.start_select(key);
        }
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                if col > 0 {
                    self.doc.col = line[..col].char_indices().last().map_or(0, |(i, _)| i);
                }
                if let Some(bi) = self.block_insert.take() {
                    self.doc.col = col;
                    self.finish_block_insert(bi);
                }
            }
            _ if ctrl(&key, 'c') => {
                self.mode = Mode::Normal;
                self.block_insert = None;
            }
            _ if ctrl(&key, 'r') => self.waiting = Some(Waiting::Register),
            _ if ctrl(&key, 'o') => {
                // What was typed so far is a step of its own, as Vim breaks
                // the undo there, so the command can undo it.
                if let Some(group) = self.group.take()
                    && group.text != self.doc.text
                {
                    self.undo.push(group);
                    self.redo.clear();
                }
                // The command and what is typed after it are the next.
                self.group = Some(self.snap());
                // Typing into a block ends here: the command may move rows.
                self.block_insert = None;
                self.waiting = Some(Waiting::OneCommand { at: (row, col) });
                self.mode = Mode::Normal;
            }
            _ if ctrl(&key, 't') || ctrl(&key, 'd') => {
                // The line indented or outdented, the cursor kept on its
                // character; typing into a block goes into this row alone.
                self.block_insert = None;
                let width = self.doc.opts.shiftwidth;
                let by = self.indent_line(row, ctrl(&key, 'd'), width);
                self.doc.col = col.saturating_add_signed(by);
            }
            _ if ctrl(&key, 'w') => {
                // The word before the cursor, and the blanks after it.
                let before = &line[..col];
                let end = before.trim_end().len();
                let mut start = end;
                if let Some(k) = before[..end].chars().last().map(|c| class(c, false)) {
                    for (i, c) in before[..end].char_indices().rev() {
                        if class(c, false) != k {
                            break;
                        }
                        start = i;
                    }
                }
                if start < col {
                    self.replace((row, start), (row, col), "");
                    self.doc.col = start;
                } else if col == 0 && row > 0 {
                    self.join_back();
                }
            }
            _ if ctrl(&key, 'u') => {
                let start = first_non_blank(&line).min(col);
                let start = if start == col { 0 } else { start };
                self.replace((row, start), (row, col), "");
                self.doc.col = start;
            }
            _ if ctrl(&key, 'h') => self.backspace(),
            KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => {
                self.type_text(c.encode_utf8(&mut [0; 4]));
            }
            KeyCode::Enter => {
                let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
                let indent = if indent.len() >= col {
                    ""
                } else {
                    indent.as_str()
                };
                let rest = line[col..].trim_start().to_string();
                self.doc.text.set_lines(
                    row,
                    row + 1,
                    &[line[..col].to_string(), format!("{indent}{rest}")],
                );
                self.doc.row = row + 1;
                self.doc.col = indent.len();
            }
            KeyCode::Tab => self.type_text(&" ".repeat(self.doc.opts.shiftwidth)),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => {
                if col < line.len() {
                    let next = line[col..].chars().next().map_or(0, char::len_utf8);
                    self.replace((row, col), (row, col + next), "");
                } else if row + 1 < self.doc.text.len() {
                    self.replace((row, col), (row + 1, 0), "");
                }
            }
            KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Home
            | KeyCode::End
                if self.block_insert.take().is_some() =>
            {
                // Moving while typing into a block ends the typing into
                // every row, as in Vim; the key moves as usual.
                self.insert_key(key);
            }
            KeyCode::Left => {
                self.doc.col = line[..col].char_indices().last().map_or(0, |(i, _)| i);
            }
            KeyCode::Right => {
                self.doc.col = line[col..]
                    .chars()
                    .next()
                    .map_or(col, |c| col + c.len_utf8());
            }
            KeyCode::Up | KeyCode::Down => {
                let m = if key.code == KeyCode::Up {
                    Motion::Up
                } else {
                    Motion::Down
                };
                if let Some(p) = self.target(m, 1, false) {
                    self.doc.row = p.0;
                    self.doc.col = p.1;
                }
            }
            KeyCode::Home => self.doc.col = 0,
            KeyCode::End => self.doc.col = line.len(),
            _ => {}
        }
    }

    // ── Replace mode ──────────────────────────────────────────────────────

    /// Text typed over what is under the cursor, a character at a time
    /// (past the line's end, added), as Replace mode types it; a newline
    /// breaks the line without taking a character.
    pub fn type_over(&mut self, text: &str) {
        for c in text.chars() {
            let (row, col) = (self.doc.row, self.doc.col);
            let line = self.line();
            if c == '\n' {
                self.doc
                    .text
                    .set_lines(row, row + 1, &[&line[..col], &line[col..]]);
                self.doc.row = row + 1;
                self.doc.col = 0;
                self.over.push(Over::Break);
                continue;
            }
            let next = line[col..].chars().next();
            let end = col + next.map_or(0, char::len_utf8);
            self.replace((row, col), (row, end), c.encode_utf8(&mut [0; 4]));
            self.doc.col = col + c.len_utf8();
            self.over.push(next.map_or(Over::Added, Over::Char));
        }
    }

    /// Backspace in Replace mode: what the last key typed over put back,
    /// else the cursor one left, changing nothing, as Vim's does.
    fn back_over(&mut self) {
        let (row, col) = (self.doc.row, self.doc.col);
        let line = self.line();
        let Some(last) = self.over.pop() else {
            self.doc.col = line[..col].char_indices().last().map_or(0, |(i, _)| i);
            return;
        };
        self.typed_over.pop();
        if last == Over::Break {
            return self.join_back();
        }
        let start = line[..col].char_indices().last().map_or(0, |(i, _)| i);
        let back = match last {
            Over::Char(c) => c.to_string(),
            _ => String::new(),
        };
        self.replace((row, start), (row, col), &back);
        self.doc.col = start;
    }

    /// Text pasted in Replace mode: typed over as keys are, and typed
    /// again for `R`'s count.
    pub fn paste_over(&mut self, text: &str) {
        self.forget_over_if_moved();
        self.type_over(text);
        self.typed_over.push_str(text);
        self.over_end = (self.doc.row, self.doc.col);
    }

    /// `R`'s count typed out now, Replace mode going on, so where the
    /// cursor is typing is where it will be once Replace mode is left.
    pub fn settle_replace_count(&mut self) {
        if self.mode != Mode::Replace || self.over_count <= 1 {
            return;
        }
        self.forget_over_if_moved();
        let again = self.typed_over.repeat(self.over_count - 1);
        self.over_count = 1;
        self.type_over(&again);
        self.over_end = (self.doc.row, self.doc.col);
    }

    fn forget_over_if_moved(&mut self) {
        if (self.doc.row, self.doc.col) != self.over_end {
            self.over.clear();
            self.typed_over.clear();
        }
    }

    fn replace_key(&mut self, key: Key) {
        self.forget_want(key);
        self.forget_over_if_moved();
        self.replace_key_here(key);
        self.over_end = (self.doc.row, self.doc.col);
    }

    fn replace_key_here(&mut self, key: Key) {
        let (row, col) = (self.doc.row, self.doc.col);
        let line = self.line();
        let moved = |me: &mut Self| {
            me.over.clear();
            me.typed_over.clear();
        };
        match key.code {
            KeyCode::Esc => {
                let again = self.typed_over.repeat(self.over_count.saturating_sub(1));
                self.type_over(&again);
                self.leave_replace();
            }
            _ if ctrl(&key, 'c') => self.leave_replace(),
            _ if ctrl(&key, 'h') => self.back_over(),
            _ if ctrl(&key, 'w') || ctrl(&key, 'u') => {
                let before = &line[..col];
                let start = if ctrl(&key, 'u') {
                    let fnb = first_non_blank(&line).min(col);
                    if fnb == col { 0 } else { fnb }
                } else {
                    let end = before.trim_end().len();
                    let k = before[..end].chars().last().map(|c| class(c, false));
                    before[..end]
                        .char_indices()
                        .rev()
                        .take_while(|&(_, c)| Some(class(c, false)) == k)
                        .last()
                        .map_or(end, |(i, _)| i)
                };
                while self.doc.row == row && self.doc.col > start {
                    self.back_over();
                }
            }
            KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => {
                self.type_over(c.encode_utf8(&mut [0; 4]));
                self.typed_over.push(c);
            }
            KeyCode::Enter => {
                self.type_over("\n");
                self.typed_over.push('\n');
            }
            KeyCode::Tab => {
                let spaces = " ".repeat(self.doc.opts.shiftwidth);
                self.type_over(&spaces);
                self.typed_over.push_str(&spaces);
            }
            KeyCode::Backspace => self.back_over(),
            KeyCode::Delete => {
                if col < line.len() {
                    let next = line[col..].chars().next().map_or(0, char::len_utf8);
                    self.replace((row, col), (row, col + next), "");
                }
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                moved(self);
                let m = match key.code {
                    KeyCode::Left => Motion::Left,
                    KeyCode::Right => Motion::Right,
                    KeyCode::Up => Motion::Up,
                    _ => Motion::Down,
                };
                if let Some(p) = self.target(m, 1, false) {
                    self.doc.row = p.0;
                    self.doc.col = p.1;
                }
            }
            KeyCode::Home => {
                moved(self);
                self.doc.col = 0;
            }
            KeyCode::End => {
                moved(self);
                self.doc.col = line.len();
            }
            _ => {}
        }
    }

    /// Out of Replace mode, the cursor a column left as Insert mode's Esc
    /// leaves it.
    fn leave_replace(&mut self) {
        self.mode = Mode::Normal;
        self.over.clear();
        self.typed_over.clear();
        let line = self.line();
        let col = self.doc.col.min(line.len());
        if col > 0 {
            self.doc.col = line[..col].char_indices().last().map_or(0, |(i, _)| i);
        }
    }

    fn backspace(&mut self) {
        let (row, col) = (self.doc.row, self.doc.col);
        let line = self.line();
        if col > 0 {
            let start = line[..col].char_indices().last().map_or(0, |(i, _)| i);
            self.replace((row, start), (row, col), "");
            self.doc.col = start;
        } else if row > 0 {
            self.join_back();
        }
    }

    fn join_back(&mut self) {
        let row = self.doc.row;
        let prev = self.doc.line(row - 1);
        let col = prev.len();
        self.replace((row - 1, col), (row, 0), "");
        self.doc.row = row - 1;
        self.doc.col = col;
    }

    // ── Visual mode ───────────────────────────────────────────────────────

    /// The selection, its start first, its end exclusive.
    pub fn selection(&self) -> Option<Selection> {
        let Mode::Visual { line, block, .. } = self.mode else {
            return None;
        };
        let cursor = (self.doc.row, self.doc.col);
        if block {
            return Some(self.block_selection());
        }
        let (a, b) = if cursor < self.anchor {
            (cursor, self.anchor)
        } else {
            (self.anchor, cursor)
        };
        let l = self.doc.line(b.0);
        let end = l[b.1.min(l.len())..]
            .chars()
            .next()
            .map_or(l.len(), |c| b.1 + c.len_utf8());
        Some(Selection {
            start: a,
            end: (b.0, end),
            linewise: line,
            block: None,
        })
    }

    /// A block's rows and columns, from the anchor's column and the
    /// cursor's (the one j and k keep, past a shorter line), to each line's
    /// end after `$`.
    fn block_selection(&self) -> Selection {
        let row = self.doc.row;
        let anchor_col = self.anchor_col;
        let cursor_col = self.block_cursor_col();
        let to_end = self.want == Some(usize::MAX);
        let block = Block {
            cols: (anchor_col.min(cursor_col), anchor_col.max(cursor_col)),
            to_end,
        };
        let (a, b) = (self.anchor.0.min(row), self.anchor.0.max(row));
        let edge = |r: usize, end: bool| {
            let line = self.doc.line(r);
            let (s, e) = block.span(&line).unwrap_or((line.len(), line.len()));
            if end { e } else { s }
        };
        Selection {
            start: (a, edge(a, false)),
            end: (b, edge(b, true)),
            linewise: false,
            block: Some(block),
        }
    }

    fn selected(&self) -> Option<Range> {
        let sel = self.selection()?;
        if let Some(block) = sel.block {
            return Some(Range::Block((sel.start.0, sel.end.0), block));
        }
        Some(if sel.linewise {
            Range::Lines(sel.start.0, sel.end.0)
        } else {
            Range::Chars(sel.start, sel.end)
        })
    }

    /// Leaves Visual mode, the cursor where the selection started when it
    /// was acted on.
    pub fn leave_visual(&mut self) {
        self.mode = Mode::Normal;
        self.keys.clear();
    }

    /// Deletes the selection, as Cmd-X does after it is copied; whether
    /// there was one.
    pub fn cut(&mut self) -> bool {
        let Some(range) = self.selected() else {
            return false;
        };
        self.mode = Mode::Normal;
        self.apply(Op::Delete, range, 1);
        true
    }

    /// Deletes the selection and starts typing where it was, as a paste
    /// over it does: at a block's own column on its first row, padded to
    /// reach it, which the cursor can't sit on past a line's end.
    pub fn cut_to_type(&mut self) {
        let block = self
            .selection()
            .and_then(|s| s.block.map(|b| (s.start.0, b.cols.0)));
        self.cut();
        let Some((row, col)) = block else {
            let at = self.doc.col;
            return self.insert_at(at);
        };
        let mut line = self.doc.line(row);
        let n = line.chars().count();
        if n < col {
            line.push_str(&" ".repeat(col - n));
            self.doc.set_line(row, &line);
        }
        self.doc.row = row;
        self.insert_at(Self::byte_of(&line, col));
    }

    /// The selection's text, for Ctrl-C.
    pub fn selected_text(&self) -> Option<String> {
        Some(match self.selected()? {
            Range::Chars(a, b) => self.text_between(a, b),
            Range::Lines(a, b) => format!(
                "{}\n",
                (a..=b)
                    .map(|r| self.doc.line(r))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            Range::Block(rows, block) => self.block_text(rows, block),
        })
    }

    fn visual_key(&mut self, key: Key) {
        let Mode::Visual {
            line: linewise,
            select,
            block,
        } = self.mode
        else {
            return;
        };
        // In Select mode, typing replaces the selection.
        if select {
            let typed_char = typed(&key).filter(|_| !key.mods.ctrl);
            match key.code {
                _ if typed_char.is_some() => {
                    let c = typed_char.unwrap_or(' ');
                    if let Some(range) = self.selected() {
                        self.mode = Mode::Normal;
                        let at = match range {
                            Range::Chars(a, _) => a,
                            Range::Lines(a, _) | Range::Block((a, _), _) => (a, 0),
                        };
                        self.apply(Op::Delete, range, 1);
                        if matches!(range, Range::Lines(..)) {
                            self.doc.text.set_lines(at.0, at.0, &[""]);
                        }
                        self.doc.row = at.0;
                        self.doc.col = at.1;
                        self.mode = Mode::Insert;
                        self.type_text(c.encode_utf8(&mut [0; 4]));
                    }
                    return;
                }
                KeyCode::Backspace | KeyCode::Delete => {
                    if let Some(range) = self.selected() {
                        self.mode = Mode::Normal;
                        self.apply(Op::Delete, range, 1);
                    }
                    return;
                }
                KeyCode::Esc => {
                    self.mode = Mode::Normal;
                    return;
                }
                KeyCode::Left
                | KeyCode::Right
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Home
                | KeyCode::End
                    if !key.mods.shift =>
                {
                    // Plain arrows end it, as `keymodel`'s `stopsel` does.
                    self.mode = Mode::Normal;
                    return self.normal_key(key);
                }
                _ => {}
            }
        }
        if key.code == KeyCode::Esc {
            self.leave_visual();
            return;
        }
        if key.mods.shift
            && matches!(
                key.code,
                KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Home
                    | KeyCode::End
            )
        {
            let plain = Key::new(key.code, crate::mux::input::Mods::NONE);
            self.keys.clear();
            self.keys.push(plain);
            if let Parse::Done(Command::Move(m, n)) = self.parse() {
                self.visual_move(m, n, block);
            }
            self.keys.clear();
            return;
        }
        self.keys.push(key);
        let first = typed(&self.keys[0]);
        let count = 1;
        // What acts on the selection, or a motion that moves its end.
        let act = |me: &mut Self, op: Op| {
            if let Some(range) = me.selected() {
                me.mode = Mode::Normal;
                me.keys.clear();
                me.apply(op, range, count);
            }
        };
        if self.keys.len() == 1 {
            match first {
                Some('d' | 'x' | 'X' | 'D') => return act(self, Op::Delete),
                Some('c' | 's' | 'S' | 'C') => return act(self, Op::Change),
                Some('y' | 'Y') => return act(self, Op::Yank),
                Some('>') => return act(self, Op::Indent),
                Some('<') => return act(self, Op::Outdent),
                Some('~') => return act(self, Op::Toggle),
                Some('u') => return act(self, Op::Lower),
                Some('U') => return act(self, Op::Upper),
                Some('o') if block => {
                    // The opposite corner: rows and columns swap.
                    let mine = self.block_cursor_col();
                    let to_end = self.want == Some(usize::MAX);
                    let row = self.doc.row;
                    self.doc.row = self.anchor.0;
                    self.place_block_corners(mine, to_end);
                    self.anchor.0 = row;
                    self.anchor.1 = Self::byte_of(&self.doc.line(row), self.anchor_col);
                    self.keys.clear();
                    return;
                }
                Some('o') => {
                    let cursor = (self.doc.row, self.doc.col);
                    (self.doc.row, self.doc.col) = self.anchor;
                    self.anchor = cursor;
                    self.want = None;
                    self.keys.clear();
                    return;
                }
                Some('O') if block => {
                    // The other corner on the same row: the columns swap.
                    let mine = self.block_cursor_col();
                    let to_end = self.want == Some(usize::MAX);
                    self.place_block_corners(mine, to_end);
                    self.keys.clear();
                    return;
                }
                Some('I' | 'A') if block => {
                    if let Some(Range::Block((a, b), blk)) = self.selected() {
                        self.keys.clear();
                        self.block_type_in(first == Some('A'), (a, b), blk);
                    }
                    return;
                }
                Some('v') | Some('V') => {
                    let to_line = first == Some('V');
                    if !block && to_line == linewise {
                        self.leave_visual();
                    } else {
                        self.mode = Mode::Visual {
                            line: to_line,
                            select: false,
                            block: false,
                        };
                        self.keys.clear();
                    }
                    return;
                }
                Some('J') => {
                    if let Some(
                        Range::Lines(a, b) | Range::Chars((a, _), (b, _)) | Range::Block((a, b), _),
                    ) = self.selected()
                    {
                        self.mode = Mode::Normal;
                        self.keys.clear();
                        self.doc.row = a;
                        self.join(b - a + 1, true);
                    }
                    return;
                }
                Some('p' | 'P') => {
                    // The selection replaced by what is put, which stays
                    // in the register for the next.
                    if let Some(range) = self.selected() {
                        let reg = self.to_put();
                        self.mode = Mode::Normal;
                        self.keys.clear();
                        let clipboard = std::mem::replace(&mut self.clipboard, false);
                        self.apply(Op::Delete, range, 1);
                        self.clipboard = clipboard;
                        self.register = reg;
                        match range {
                            Range::Block((a, _), blk) => {
                                // At the block's own column, which the
                                // cursor can't sit on past a line's end.
                                let reg = self.register.clone();
                                if reg.block {
                                    self.put_block(&reg.text, a, blk.cols.0, 1);
                                } else if reg.linewise {
                                    self.block_corner(a, blk);
                                    self.put(true, 1);
                                } else {
                                    let mut line = self.doc.line(a);
                                    let n = line.chars().count();
                                    if n < blk.cols.0 {
                                        line.push_str(&" ".repeat(blk.cols.0 - n));
                                        self.doc.set_line(a, &line);
                                    }
                                    let at = Self::byte_of(&line, blk.cols.0);
                                    let end = self.replace((a, at), (a, at), &reg.text);
                                    let l = self.doc.line(end.0);
                                    let last =
                                        l[..end.1].char_indices().last().map_or(0, |(i, _)| i);
                                    self.set(end.0, last);
                                }
                            }
                            Range::Lines(a, _) => {
                                let len = self.doc.text.len();
                                if a < len && !(len == 1 && self.doc.line(0).is_empty()) {
                                    self.doc.row = a;
                                    self.put(true, 1);
                                } else {
                                    self.doc.row = len - 1;
                                    self.put(false, 1);
                                }
                            }
                            Range::Chars(a, _) => {
                                self.doc.row = a.0;
                                self.doc.col = a.1;
                                self.put(true, 1);
                            }
                        }
                    }
                    return;
                }
                Some(':') | Some('/') | Some('?') => {
                    self.keys.clear();
                    if first != Some(':') {
                        self.out.push(Effect::Prompt(first.unwrap_or('/')));
                    }
                    return;
                }
                _ => {}
            }
        }
        // A text object extends the selection over it.
        if let Some(c @ ('i' | 'a')) = first {
            if self.keys.len() < 2 {
                return;
            }
            let what = typed(&self.keys[1]);
            self.keys.clear();
            if let Some(what) = what
                && let Some(range) = self.object(c == 'i', what, 1)
            {
                match range {
                    Range::Chars(a, b) => {
                        self.anchor = a;
                        self.anchor_col = Self::char_index(&self.doc.line(a.0), a.1);
                        let l = self.doc.line(b.0);
                        let last = l[..b.1.min(l.len())]
                            .char_indices()
                            .last()
                            .map_or(0, |(i, _)| i);
                        self.doc.row = b.0;
                        self.doc.col = last;
                    }
                    Range::Block(..) => {}
                    Range::Lines(a, b) => {
                        self.anchor = (a, 0);
                        self.mode = Mode::Visual {
                            line: true,
                            select,
                            block: false,
                        };
                        self.doc.row = b;
                    }
                }
            }
            return;
        }
        if first == Some('r') {
            if self.keys.len() < 2 {
                return;
            }
            let with = typed(&self.keys[1]);
            self.keys.clear();
            if let (Some(c), Some(Range::Block((a, b), blk))) = (with, self.selected()) {
                for r in a..=b {
                    let line = self.doc.line(r);
                    if let Some((s, e)) = blk.span(&line) {
                        let over: String = line[s..e].chars().map(|_| c).collect();
                        self.replace((r, s), (r, e), &over);
                    }
                }
                self.mode = Mode::Normal;
                self.block_corner(a, blk);
                return;
            }
            if let (Some(c), Some(range)) = (with, self.selected()) {
                let (a, b) = match range {
                    Range::Chars(a, b) => (a, b),
                    Range::Lines(a, b) => ((a, 0), (b, self.doc.line(b).len())),
                    Range::Block(..) => return,
                };
                let text = self.text_between(a, b);
                let replaced: String = text
                    .chars()
                    .map(|x| if x == '\n' { x } else { c })
                    .collect();
                self.replace(a, b, &replaced);
                self.mode = Mode::Normal;
                self.set(a.0, a.1);
            }
            return;
        }
        match self.parse() {
            Parse::More => {}
            Parse::Invalid => self.keys.clear(),
            Parse::Done(Command::Move(m, count)) => {
                self.keys.clear();
                self.visual_move(m, count, block);
            }
            Parse::Done(Command::Simple('\0', _)) => {
                let k = self.keys.pop();
                self.keys.clear();
                match k {
                    Some(k) if ctrl(&k, 'c') => self.leave_visual(),
                    // Ctrl-V: to a block, or out of one, as v and V do.
                    Some(k) if ctrl(&k, 'v') && block => self.leave_visual(),
                    Some(k) if ctrl(&k, 'v') => {
                        self.end_from = Self::char_index(&self.line(), self.doc.col);
                        self.anchor_col =
                            Self::char_index(&self.doc.line(self.anchor.0), self.anchor.1);
                        self.mode = Mode::Visual {
                            line: false,
                            select: false,
                            block: true,
                        };
                    }
                    _ => {}
                }
            }
            Parse::Done(_) => self.keys.clear(),
        }
    }

    /// The cursor put on the block's own column, where j and k left it
    /// wanting one past a shorter line, so swapping corners keeps it.
    fn block_cursor_col(&self) -> usize {
        match self.want {
            Some(usize::MAX) => self.end_from,
            Some(w) => w,
            None => Self::char_index(&self.line(), self.doc.col),
        }
    }

    /// A block's columns swapped between the anchor and the cursor (on
    /// the cursor's row as it now is), the cursor's kept in `want` past a
    /// shorter line, and a block to each line's end kept so.
    fn place_block_corners(&mut self, mine: usize, to_end: bool) {
        let theirs = self.anchor_col;
        self.anchor_col = mine;
        self.anchor.1 = Self::byte_of(&self.doc.line(self.anchor.0), mine);
        if to_end {
            self.end_from = theirs;
        } else {
            self.want = Some(theirs);
        }
        let line = self.line();
        self.doc.col = Self::byte_of(&line, theirs);
        self.clamp();
    }

    /// `I` and `A` on a block: typing on its first row before or after
    /// it, into every row on Esc.
    fn block_type_in(&mut self, after: bool, (a, b): (usize, usize), blk: Block) {
        let col = if after { blk.cols.1 + 1 } else { blk.cols.0 };
        let skip = if after {
            Vec::new()
        } else {
            self.rows_short_of((a, b), col)
        };
        self.start_block_insert((a, b), col, after && blk.to_end, skip);
    }

    /// A motion moving the selection's end, keeping the column j and k
    /// want (a block's own) and, for a block stretched by `$`, the column
    /// it was stretched from.
    fn visual_move(&mut self, m: Motion, count: usize, block: bool) {
        if block && m == Motion::LineEnd && self.want != Some(usize::MAX) {
            self.end_from = self
                .want
                .unwrap_or_else(|| Self::char_index(&self.line(), self.doc.col));
        }
        if !matches!(m, Motion::Down | Motion::Up | Motion::LineEnd) {
            self.want = None;
        }
        if let Some(p) = self.target(m, count.max(1), count > 0) {
            self.doc.row = p.0;
            self.doc.col = p.1;
        }
    }

    /// Selects from one position to another by characters, as dragging
    /// the mouse does: Select mode in easy mode, Visual mode else.
    pub fn select(&mut self, from: (usize, usize), to: (usize, usize)) {
        self.anchor = from;
        self.mode = Mode::Visual {
            line: false,
            select: self.easy,
            block: false,
        };
        self.doc.row = to.0;
        self.doc.col = to.1;
        self.clamp();
    }

    /// An Ex command typed after `:`.
    pub fn ex(&mut self, command: &str) {
        let command = command.trim();
        if let Ok(n) = command.parse::<usize>() {
            let r = n.saturating_sub(1).min(self.doc.text.len() - 1);
            let fnb = first_non_blank(&self.doc.line(r));
            self.set(r, fnb);
            return;
        }
        static SUB: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"^(%)?s/((?:[^/\\]|\\.)*)/((?:[^/\\]|\\.)*)/?(g)?$").unwrap()
        });
        if let Some(c) = SUB.captures(command) {
            let all_lines = c.get(1).is_some();
            let Some(re) = Self::pattern(&c[2]) else {
                return;
            };
            let with = c[3].replace("\\/", "/").replace('&', "$0");
            let global = c.get(4).is_some();
            let rows: Vec<usize> = if all_lines {
                (0..self.doc.text.len()).collect()
            } else {
                vec![self.doc.row]
            };
            let mut found = false;
            for r in rows {
                let line = self.doc.line(r);
                let new = if global {
                    re.replace_all(&line, with.as_str()).into_owned()
                } else {
                    re.replace(&line, with.as_str()).into_owned()
                };
                if new != line {
                    found = true;
                    self.doc.set_line(r, &new);
                }
            }
            if !found {
                self.say(format!("E486: Pattern not found: {}", &c[2]));
            }
            return;
        }
        match command {
            "w" | "w!" | "up" | "update" => self.out.push(Effect::Save),
            "q" | "q!" | "qa" | "qa!" | "quit" => self.out.push(Effect::Quit),
            "wq" | "x" | "wqa" | "xa" => self.out.push(Effect::SaveQuit),
            "noh" | "nohlsearch" => {}
            "" => {}
            other => self.say(format!("E492: Not an editor command: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::input::Mods;

    fn editor(lines: &[&str]) -> Editor {
        Editor::new(Text::from_lines(lines))
    }

    /// Types keys as Vim's notation writes them: plain characters, and
    /// <Esc>, <CR>, <BS>, <C-r> and the like.
    fn press(e: &mut Editor, keys: &str) {
        let mut rest = keys;
        while !rest.is_empty() {
            let key = if rest.starts_with('<')
                && let Some(end) = rest.find('>')
            {
                let name = &rest[1..end];
                rest = &rest[end + 1..];
                match name {
                    "Esc" => Key::new(KeyCode::Esc, Mods::NONE),
                    "CR" => Key::new(KeyCode::Enter, Mods::NONE),
                    "BS" => Key::new(KeyCode::Backspace, Mods::NONE),
                    "Tab" => Key::new(KeyCode::Tab, Mods::NONE),
                    "S-Right" => Key::new(KeyCode::Right, Mods::SHIFT),
                    n if n.starts_with("C-") => {
                        Key::new(KeyCode::Char(n.chars().nth(2).unwrap()), Mods::CTRL)
                    }
                    other => panic!("no key {other}"),
                }
            } else {
                let c = rest.chars().next().unwrap();
                rest = &rest[c.len_utf8()..];
                Key::new(KeyCode::Char(c), Mods::NONE)
            };
            e.begin();
            e.key(key);
            e.end();
        }
    }

    fn lines(e: &Editor) -> Vec<String> {
        e.doc.text.lines()
    }

    #[test]
    fn a_new_count_replaces_the_changes_count_whole() {
        let keys = |s: &str| -> Vec<Key> {
            s.chars()
                .map(|c| match c {
                    '⎋' => Key::new(KeyCode::Esc, Mods::NONE),
                    '→' => Key::new(KeyCode::Right, Mods::SHIFT),
                    '↵' => Key::new(KeyCode::Enter, Mods::NONE),
                    c => Key::new(KeyCode::Char(c), Mods::NONE),
                })
                .collect()
        };
        for (before, after) in [
            ("2dw", "4dw"),
            ("d3w", "4dw"),
            ("2d3w", "4dw"),
            ("x", "4x"),
            ("A!⎋", "4A!⎋"),
            ("g~2w", "4g~w"),
            ("d0", "4d0"),
        ] {
            assert_eq!(recount(&keys(before), 4), Some(keys(after)), "{before}");
        }
        let ctrl_a = [Key::new(KeyCode::Char('a'), Mods::CTRL)];
        let mut four_ctrl_a = keys("4");
        four_ctrl_a.extend(ctrl_a);
        assert_eq!(recount(&ctrl_a, 4), Some(four_ctrl_a));
        for kept in ["→x⎋", "↵", " b", "I!⎋", "a!⎋", "i!⎋", "\"+p", "vex", ""] {
            assert_eq!(recount(&keys(kept), 4), None, "{kept}");
        }
    }

    #[test]
    fn take_count_takes_only_a_count() {
        let mut e = editor(&["one"]);
        press(&mut e, "12");
        assert_eq!(e.take_count(), Some(12));
        assert!(!e.pending());
        press(&mut e, "2d");
        assert_eq!(e.take_count(), None);
        assert!(e.pending(), "the operator left waiting");
    }

    #[test]
    fn motions_move_as_vims_do() {
        let mut e = editor(&["alpha beta-gamma delta", "", "  second line here"]);
        press(&mut e, "w");
        assert_eq!(e.col(), 6);
        press(&mut e, "w");
        assert_eq!(e.col(), 10, "punctuation is a word of its own");
        press(&mut e, "W");
        assert_eq!(e.col(), 17);
        press(&mut e, "w");
        assert_eq!((e.row(), e.col()), (1, 0), "an empty line is a word");
        press(&mut e, "w");
        assert_eq!((e.row(), e.col()), (2, 2));
        press(&mut e, "b");
        assert_eq!((e.row(), e.col()), (1, 0));
        press(&mut e, "gg");
        press(&mut e, "e");
        assert_eq!(e.col(), 4);
        press(&mut e, "$");
        assert_eq!(e.col(), 21);
        press(&mut e, "0fd");
        assert_eq!(e.col(), 17);
        press(&mut e, "0;");
        assert_eq!(e.col(), 17, "; finds the same again");
        press(&mut e, "G");
        assert_eq!(
            (e.row(), e.col()),
            (2, 2),
            "G goes to the last line's first word"
        );
        press(&mut e, "2G");
        assert_eq!(e.row(), 1);
        press(&mut e, "kll");
        assert_eq!(e.col(), 2);
        press(&mut e, "j");
        assert_eq!((e.row(), e.col()), (1, 0));
        press(&mut e, "j");
        assert_eq!(
            e.col(),
            2,
            "j keeps the column it wants across a shorter line"
        );
    }

    #[test]
    fn ctrl_a_and_ctrl_x_change_the_number_at_or_after_the_cursor() {
        let mut e = editor(&["item 9 of 10", "no number", "-1 and 007", "on 2026-11-12"]);
        press(&mut e, "<C-a>");
        assert_eq!(
            e.doc.line(0),
            "item 10 of 10",
            "the first number after the cursor"
        );
        assert_eq!(e.col(), 6, "the cursor on its last digit");
        press(&mut e, "w");
        press(&mut e, "w");
        press(&mut e, "5<C-x>");
        assert_eq!(e.doc.line(0), "item 10 of 5", "a count subtracts that many");
        press(&mut e, "u");
        assert_eq!(e.doc.line(0), "item 10 of 10", "one step to undo");
        press(&mut e, "j<C-a>");
        assert_eq!(
            e.doc.line(1),
            "no number",
            "a line with no number is left alone"
        );
        press(&mut e, "j0<C-a>");
        assert_eq!(
            e.doc.line(2),
            "0 and 007",
            "a sign at the line's start, and 0 has none"
        );
        press(&mut e, "<C-x>");
        assert_eq!(e.doc.line(2), "-1 and 007");
        press(&mut e, "w<C-a>");
        assert_eq!(e.doc.line(2), "-1 and 008", "leading zeros keep the width");
        press(&mut e, "j0f-<C-a>");
        assert_eq!(e.doc.line(3), "on 2026-12-12", "a date's dash is no sign");

        let mut e = editor(&["heading", "item 9", "abcdefgh"]);
        press(&mut e, "j<C-a>j");
        assert_eq!(
            (e.row(), e.col()),
            (2, 6),
            "the next line down keeps the number's column"
        );
    }

    #[test]
    fn operators_take_counts_motions_and_text_objects_and_undo_takes_them_back() {
        let mut e = editor(&["one two three four", "five", "six"]);
        press(&mut e, "dw");
        assert_eq!(e.doc.line(0), "two three four");
        press(&mut e, "2dw");
        assert_eq!(e.doc.line(0), "four");
        press(&mut e, "u");
        assert_eq!(e.doc.line(0), "two three four");
        press(&mut e, "u");
        assert_eq!(
            e.doc.line(0),
            "one two three four",
            "each command undone on its own"
        );
        press(&mut e, "<C-r>");
        assert_eq!(e.doc.line(0), "two three four");
        press(&mut e, "wciwTHREE<Esc>");
        assert_eq!(e.doc.line(0), "two THREE four");
        press(&mut e, "u");
        assert_eq!(
            e.doc.line(0),
            "two three four",
            "a change and what was typed are one step"
        );
        press(&mut e, "jdd");
        assert_eq!(lines(&e), ["two three four", "six"]);
        press(&mut e, "p");
        assert_eq!(
            lines(&e),
            ["two three four", "six", "five"],
            "a deleted line put back under"
        );
        press(&mut e, "ggyyP");
        assert_eq!(lines(&e)[0..2], ["two three four", "two three four"]);
        press(&mut e, "3x");
        assert_eq!(e.doc.line(0), " three four");
        press(&mut e, "A!<Esc>");
        assert_eq!(e.doc.line(0), " three four!");
        press(&mut e, "0daw");
        assert_eq!(e.doc.line(0), " four!", "a word and the blank after it");
        press(&mut e, ">>");
        assert_eq!(e.doc.line(0), "   four!", "indented by shiftwidth");
        press(&mut e, "gUU");
        assert_eq!(e.doc.line(0), "   FOUR!");
    }

    #[test]
    fn insert_mode_types_joins_and_carries_the_indent() {
        let mut e = editor(&["  item"]);
        press(&mut e, "A one<CR>two<Esc>");
        assert_eq!(
            lines(&e),
            ["  item one", "  two"],
            "Enter keeps the line's indent"
        );
        press(&mut e, "I<BS><BS><BS><Esc>");
        assert_eq!(
            lines(&e),
            ["  item onetwo"],
            "Backspace at a line's start joins it to the one above"
        );
        press(&mut e, "otext (inside) end<Esc>F(di(");
        assert_eq!(e.doc.line(1), "  text () end");
        press(&mut e, "ccnew<Esc>");
        assert_eq!(e.doc.line(1), "  new", "cc keeps the indent");
        press(&mut e, "kJ");
        assert_eq!(lines(&e), ["  item onetwo new"]);
    }

    #[test]
    fn replace_mode_types_over_and_backspace_puts_back() {
        let mut e = editor(&["hello"]);
        press(&mut e, "Rab");
        assert_eq!(e.mode, Mode::Replace);
        press(&mut e, "<Esc>");
        assert_eq!(e.doc.line(0), "abllo");
        assert_eq!((e.mode, e.col()), (Mode::Normal, 1), "the cursor on b");
        press(&mut e, "u");
        assert_eq!(e.doc.line(0), "hello", "the whole of it one step to undo");
        press(&mut e, "0Rab<BS><BS><Esc>");
        assert_eq!(e.doc.line(0), "hello", "Backspace puts back what was there");
        press(&mut e, "0l<BS>Rx<BS><BS>y<Esc>");
        assert_eq!(
            e.doc.line(0),
            "yello",
            "Backspace past where R started only moves left"
        );
        press(&mut e, "u$Rxyz<Esc>");
        assert_eq!(e.doc.line(0), "hellxyz", "past the end it adds");
        press(&mut e, "u0Rab<CR>cd<Esc>");
        assert_eq!(
            lines(&e),
            ["ab", "cdo"],
            "Enter breaks the line, taking nothing"
        );
        press(&mut e, "u");

        let mut e = editor(&["abcdef"]);
        press(&mut e, "RX");
        e.doc.col = 4;
        press(&mut e, "<BS><Esc>");
        assert_eq!(
            e.doc.line(0),
            "Xbcdef",
            "moved by a click, Backspace puts back nothing it didn't type over"
        );

        let mut clicked = editor(&["one", "two"]);
        press(&mut clicked, "3RX");
        clicked.doc.row = 1;
        clicked.doc.col = 0;
        clicked.settle_replace_count();
        assert_eq!(
            lines(&clicked),
            ["Xne", "two"],
            "nothing typed again after a click"
        );

        let mut down = editor(&["abcdef", "abcdef", "abcdef"]);
        press(&mut down, "R");
        for code in [KeyCode::Down, KeyCode::Right, KeyCode::Right, KeyCode::Down] {
            down.key(Key::new(code, Mods::NONE));
        }
        press(&mut down, "X<Esc>");
        assert_eq!(
            down.doc.line(2),
            "abXdef",
            "the arrows' column, not one kept from before"
        );

        let mut e = editor(&["0123456789"]);
        press(&mut e, "3Rab<Esc>");
        assert_eq!(e.doc.line(0), "ababab6789", "a count types it again");
        assert_eq!(e.col(), 5);
    }

    #[test]
    fn insert_modes_ctrl_keys_put_a_register_run_a_command_and_indent() {
        let mut e = editor(&["alpha beta"]);
        press(&mut e, "yiwA <C-r>\"<Esc>");
        assert_eq!(e.doc.line(0), "alpha beta alpha", "the register typed in");
        e.search_for("beta", true);
        press(&mut e, "0i<C-r>/<Esc>");
        assert_eq!(e.doc.line(0), "betaalpha beta alpha", "the last search");
        press(&mut e, "u");
        press(&mut e, "i<C-r>z<Esc>");
        assert_eq!(
            e.doc.line(0),
            "alpha beta alpha",
            "an unknown register types nothing"
        );
        assert!(
            e.effects()
                .iter()
                .any(|m| matches!(m, Effect::Message(t) if t.starts_with("E353"))),
            "and says so"
        );

        let mut e = editor(&["one", "two"]);
        press(&mut e, "A<C-o>");
        assert_eq!(e.mode, Mode::Normal);
        assert!(e.one_command());
        press(&mut e, "0");
        assert_eq!((e.mode, e.col()), (Mode::Insert, 0), "back to typing");
        press(&mut e, "X<C-o><Esc>Y<Esc>");
        assert_eq!(e.doc.line(0), "XYone");
        press(&mut e, "u");
        assert_eq!(e.doc.line(0), "Xone", "Ctrl-O breaks the step to undo");
        press(&mut e, "u");
        assert_eq!(e.doc.line(0), "one");
        let mut gone = editor(&["abc", "x"]);
        press(&mut gone, "A<C-o>ddX<Esc>");
        assert_eq!(
            lines(&gone),
            ["Xx"],
            "typing goes on where the command left it"
        );
        let mut ex = editor(&["a"]);
        press(&mut ex, "A<C-o>:");
        assert!(ex.one_command(), "waiting for the prompt");
        ex.ex("s/a//");
        ex.prompt_done();
        press(&mut ex, "<Esc>");
        assert_eq!((lines(&ex), ex.mode), (vec![String::new()], Mode::Normal));
        let mut dd = editor(&["one", "two"]);
        press(&mut dd, "i<C-o>dd<Esc>u");
        assert_eq!(lines(&dd), ["one", "two"], "a command under Ctrl-O undoes");
        press(&mut e, "AZ<C-o>u");
        assert_eq!(
            (e.doc.line(0).as_str(), e.mode),
            ("one", Mode::Insert),
            "u under Ctrl-O undoes what was just typed"
        );
        press(&mut e, "<Esc>");
        let mut named = editor(&["word"]);
        press(&mut named, "yiwA <C-o>\"+p");
        assert_eq!(
            (named.doc.line(0).as_str(), named.mode),
            ("word word", Mode::Insert),
            "a register named is part of the one command"
        );
        press(&mut named, "<Esc>");
        press(&mut e, "A<C-o>l!<Esc>");
        assert_eq!(
            e.doc.line(0),
            "one!",
            "a command that doesn't move keeps the cursor past the end"
        );
        press(&mut e, "i<C-o>dd-<Esc>");
        assert_eq!(lines(&e), ["-two"], "the line deleted and typing kept on");

        let mut e = editor(&["- item", "", "yy"]);
        press(&mut e, "llli<C-t>");
        assert_eq!(
            (e.doc.line(0).as_str(), e.col()),
            ("  - item", 5),
            "on the same character"
        );
        press(&mut e, "<C-d><C-d><Esc>");
        assert_eq!(
            e.doc.line(0),
            "- item",
            "and back, nothing past the line's start"
        );
        press(&mut e, "jji<C-t><Esc>");
        assert_eq!(e.doc.line(2), "  yy");
        press(&mut e, "ggyyGo<C-r>\"<Esc>");
        assert_eq!(
            lines(&e)[3..],
            ["  - item", ""],
            "a line put with its newline"
        );
    }

    #[test]
    fn block_visual_mode_acts_on_a_rectangle() {
        let text = ["abcdef", "a", "abcdefgh", "abcd"];
        let mut e = editor(&text);
        press(&mut e, "l<C-v>jjl");
        assert!(matches!(e.mode, Mode::Visual { block: true, .. }));
        press(&mut e, "d");
        assert_eq!(
            lines(&e),
            ["adef", "a", "adefgh", "abcd"],
            "two columns cut, a row short of them left alone"
        );
        assert_eq!((e.row(), e.col()), (0, 1), "the cursor on the top-left");
        assert_eq!(e.register.text, "bc\n\nbc");
        press(&mut e, "u");
        assert_eq!(lines(&e), text);

        press(&mut e, "gg0l<C-v>jjly$p");
        assert_eq!(
            lines(&e)[..3],
            ["abcdefbc", "a     ", "abcdefbcgh"],
            "put as a column, a short row padded to reach it"
        );
        press(&mut e, "u");
        press(&mut e, "gg0<C-v>jrx");
        assert_eq!(lines(&e)[..2], ["xbcdef", "x"]);
        press(&mut e, "u");
        press(&mut e, "gg0l<C-v>jj>");
        assert_eq!(
            lines(&e)[..3],
            ["a  bcdef", "a", "a  bcdefgh"],
            "shifted at the block's column"
        );
        press(&mut e, "u");
        press(&mut e, "gg0l<C-v>jjI- <Esc>");
        assert_eq!(
            lines(&e)[..3],
            ["a- bcdef", "a", "a- bcdefgh"],
            "typed into every line"
        );
        press(&mut e, "u");
        assert_eq!(lines(&e), text, "one step to undo");
        press(&mut e, "gg0<C-v>jjj$A;<Esc>");
        assert_eq!(
            lines(&e),
            ["abcdef;", "a;", "abcdefgh;", "abcd;"],
            "each line's end"
        );
        press(&mut e, "u");
        press(&mut e, "gg0l<C-v>jjlc_<Esc>");
        assert_eq!(
            lines(&e)[..3],
            ["a_def", "a", "a_defgh"],
            "changed on each row"
        );
        press(&mut e, "u");

        let mut short = editor(&["abcdef", "xy"]);
        press(&mut short, "3l<C-v>j$d");
        assert_eq!(
            lines(&short),
            ["abc", "xy"],
            "$ keeps the block's left edge"
        );
        let mut short = editor(&["a", "abcdef"]);
        press(&mut short, "j2l<C-v>kIX<Esc>");
        assert_eq!(
            lines(&short),
            ["a", "abXcdef"],
            "typing starts on a row that reaches the block"
        );
        let mut moved = editor(&["abc", "abc"]);
        press(&mut moved, "<C-v>jIX");
        moved.key(Key::new(KeyCode::Right, Mods::NONE));
        press(&mut moved, "<Esc>");
        assert_eq!(
            lines(&moved),
            ["Xabc", "abc"],
            "moving ends typing into every row"
        );
        let mut switched = editor(&["abcd"]);
        press(&mut switched, "$v<C-v>d");
        assert_eq!(
            lines(&switched),
            ["abc"],
            "v then Ctrl-V keeps the block's column"
        );
        let mut clicked = editor(&["abc", "abc"]);
        press(&mut clicked, "<C-v>jIX");
        clicked.doc.col = 3;
        press(&mut clicked, "<Esc>");
        assert_eq!(
            lines(&clicked),
            ["Xabc", "abc"],
            "a click ends typing into every row"
        );
        let mut indented = editor(&["abcd", "abcd"]);
        press(&mut indented, "ll<C-v>jI<C-t>X<Esc>");
        assert_eq!(lines(&indented), ["  abXcd", "abcd"]);
        let mut ragged = editor(&["abc", "d", "xy", "xy"]);
        press(&mut ragged, "0<C-v>lljy");
        press(&mut ragged, "jj0l2P");
        assert_eq!(
            lines(&ragged)[2..],
            ["xabcabcy", "xd  d  y"],
            "each copy of a count padded to the block's width"
        );
        let mut pasted = editor(&["abc", "abc"]);
        press(&mut pasted, "<C-v>jI");
        pasted.paste_text("X");
        press(&mut pasted, "<Esc>");
        assert_eq!(
            lines(&pasted),
            ["Xabc", "Xabc"],
            "a paste goes into every row"
        );
        let mut back = editor(&["abcd", "abcd"]);
        press(&mut back, "ll<C-v>jI<BS>XY<Esc>");
        assert_eq!(
            lines(&back),
            ["aXYcd", "abcd"],
            "deleting back past the block's column keeps it to the row"
        );
        let mut corners = editor(&["abcdef", "a"]);
        press(&mut corners, "l<C-v>lljOd");
        assert_eq!(
            lines(&corners),
            ["aef", "a"],
            "O on a short row keeps the rectangle"
        );
        let mut corners = editor(&["abcdef", "a"]);
        press(&mut corners, "l<C-v>lljoOd");
        assert_eq!(lines(&corners), ["aef", "a"], "and so does o");
        let mut swap = editor(&["ab12", "cd34"]);
        press(&mut swap, "<C-v>jlyll<C-v>jlp");
        assert_eq!(
            lines(&swap),
            ["abab", "cdcd"],
            "p over a block at the lines' end"
        );
        let mut charwise = editor(&["a", "abcdef"]);
        charwise.register = Register {
            text: "X".into(),
            linewise: false,
            block: false,
        };
        press(&mut charwise, "j3l<C-v>kp");
        assert_eq!(
            lines(&charwise),
            ["a  X", "abcef"],
            "text put at the block's column"
        );
        let mut pasted = editor(&["abcd", "abcd"]);
        press(&mut pasted, "$<C-v>j");
        pasted.cut_to_type();
        pasted.type_text("X");
        assert_eq!(
            lines(&pasted),
            ["abcX", "abc"],
            "a paste over a block at its column"
        );
        let mut none = editor(&["abcdef", "a", "b"]);
        press(&mut none, "4l<C-v>jojjIX<Esc>");
        assert_eq!(
            lines(&none),
            ["abcdef", "a", "b"],
            "no row reaches the block"
        );
        let mut easy = editor(&["abc", "abc"]);
        easy.easy = true;
        press(&mut easy, "<C-v>jIX<S-Right><Esc>i<Esc>");
        assert_eq!(
            lines(&easy),
            ["Xabc", "abc"],
            "a selection ends typing into a block"
        );
        let mut shifted = editor(&["abc", "abc"]);
        press(&mut shifted, "<C-v>j<S-Right>d");
        assert_eq!(
            lines(&shifted),
            ["c", "c"],
            "Shift with an arrow widens a block"
        );
        let mut joined = editor(&["abc", "def", "ghi"]);
        press(&mut joined, "<C-v>jj$A");
        joined.key(Key::new(KeyCode::Delete, Mods::NONE));
        press(&mut joined, "X<Esc>");
        assert_eq!(
            lines(&joined),
            ["abcXdef", "ghi"],
            "a join ends typing into every row"
        );
        let mut cmd = editor(&["abc", "def"]);
        press(&mut cmd, "<C-v>jI<C-o>ddX<Esc>");
        assert_eq!(lines(&cmd), ["Xdef"], "Ctrl-O ends typing into every row");
        let mut two = editor(&["abc", "abc"]);
        press(&mut two, "<C-v>j$cX<Esc>");
        assert_eq!(
            lines(&two),
            ["X", "X"],
            "a row cut to nothing still takes it"
        );

        press(&mut e, "gg0l<C-v>jjlO");
        assert_eq!(
            (e.row(), e.col(), e.anchor),
            (2, 1, (0, 2)),
            "the other corner"
        );
        press(&mut e, "v");
        assert!(matches!(
            e.mode,
            Mode::Visual {
                block: false,
                line: false,
                ..
            }
        ));
        press(&mut e, "<Esc><C-v><C-v>");
        assert_eq!(e.mode, Mode::Normal, "Ctrl-V again leaves it");
    }

    #[test]
    fn visual_mode_selects_and_acts_and_search_finds_with_smartcase() {
        let mut e = editor(&["alpha beta", "gamma Delta", "epsilon"]);
        press(&mut e, "wvey");
        assert_eq!(e.register.text, "beta");
        assert_eq!(e.mode, Mode::Normal);
        press(&mut e, "Vjd");
        assert_eq!(lines(&e), ["epsilon"]);
        press(&mut e, "u");
        assert_eq!(lines(&e).len(), 3);
        e.search_for("delta", true);
        assert_eq!((e.row(), e.col()), (1, 6), "lowercase matches either case");
        e.search_for("Delta", true);
        assert_eq!((e.row(), e.col()), (1, 6));
        e.search_for("eta", true);
        assert_eq!((e.row(), e.col()), (0, 7), "wrapping round to the top");
        press(&mut e, "n");
        assert_eq!((e.row(), e.col()), (0, 7));
        e.ex("%s/a/A/g");
        assert_eq!(e.doc.line(0), "AlphA betA");
        e.ex("3");
        assert_eq!(e.row(), 2);
    }

    #[test]
    fn easy_modes_shift_arrows_select_and_typing_replaces() {
        let mut e = editor(&["hello world"]);
        e.easy = true;
        press(&mut e, "<S-Right><S-Right><S-Right><S-Right>");
        assert!(matches!(e.mode, Mode::Visual { select: true, .. }));
        press(&mut e, "J");
        assert_eq!(
            e.doc.line(0),
            "J world",
            "a key typed over a selection replaces it"
        );
        assert_eq!(e.mode, Mode::Insert);
    }
}
