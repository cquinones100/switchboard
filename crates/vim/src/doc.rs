//! The lines and the cursor the editor works on (`Doc`), what a selection
//! covers, and the two ways a program puts its own on top: a `Note`, state
//! of its own kept beside the text (the notes app's pending markers and its
//! checklist options), and its `Folding`, sections closed to one line,
//! which the motions count as one, as Vim's folds do. A plain text has
//! neither (`()`).

use std::fmt::Debug;

use super::text::Text;

/// What a program keeps beside the text, and its folds.
pub trait Note: Clone + Debug + Default {
    type Folds: Folding;

    /// A new text put in (`Editor::load`): forget what went with the old
    /// one.
    fn reset(&mut self) {}
}

impl Note for () {
    type Folds = ();
}

/// Sections closed to one line. Each method is handed the text's lines,
/// from which the sections are read.
pub trait Folding: Clone + Debug + Default {
    /// Each row shown, with how many rows a closed section hides under it.
    fn shown(&self, lines: &[String]) -> Vec<(usize, Option<usize>)> {
        (0..lines.len()).map(|r| (r, None)).collect()
    }

    /// The closed section a row is in, its first and last rows.
    fn closed_over(&self, _lines: &[String], _row: usize) -> Option<(usize, usize)> {
        None
    }

    /// Opens the sections hiding a row.
    fn open_over(&mut self, _lines: &[String], _row: usize) {}

    /// Carried across an edit.
    fn follow_edit(&mut self, _before: &[String], _after: &[String]) {}

    /// Carried across lines moved as they are.
    fn rows_moved(&mut self, _to: &dyn Fn(usize) -> usize) {}

    /// `z` and a key on a row: `c` closes, `o` opens, `a` toggles, `M`
    /// closes all, `R` opens all.
    fn z(&mut self, _lines: &[String], _row: usize, _key: char) {}
}

impl Folding for () {}

/// A selection, for what acts on one: its first and last row, the byte
/// where it starts on the first and where it ends (exclusive) on the last,
/// whether whole lines were selected, and the block's columns when it is a
/// block (Ctrl-V).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub linewise: bool,
    pub block: Option<Block>,
}

impl Selection {
    /// The bytes of a row inside the selection, the end exclusive; none
    /// for a row it misses.
    pub fn span(&self, row: usize, line: &str) -> Option<(usize, usize)> {
        if row < self.start.0 || row > self.end.0 {
            return None;
        }
        if let Some(block) = self.block {
            return block.span(line);
        }
        let s = if self.linewise || row > self.start.0 {
            0
        } else {
            self.start.1.min(line.len())
        };
        let e = if self.linewise || row < self.end.0 {
            line.len()
        } else {
            self.end.1.min(line.len())
        };
        Some((s, e.max(s)))
    }
}

/// A block selection's columns, in characters, the right one inclusive,
/// and whether it reaches each line's end (`$`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub cols: (usize, usize),
    pub to_end: bool,
}

impl Block {
    /// The bytes of a line inside the block, the end exclusive; none for a
    /// line that ends before the block's left column.
    pub fn span(&self, line: &str) -> Option<(usize, usize)> {
        let mut starts = line.char_indices().map(|(i, _)| i).skip(self.cols.0);
        let from = starts.next()?;
        let to = if self.to_end {
            line.len()
        } else {
            starts.nth(self.cols.1 - self.cols.0).unwrap_or(line.len())
        };
        Some((from, to))
    }
}

/// What Insert mode was asked for by an edit: at the cursor, or after the
/// line's end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Insert {
    Here,
    End,
}

/// The lines and the cursor in them, as the editor (and a program's own
/// edits, through `Editor::change`) change them.
#[derive(Clone, Debug)]
pub struct Doc<N: Note = ()> {
    pub text: Text,
    pub row: usize,
    pub col: usize,
    /// Typing asked for by the edit just made.
    pub insert: Option<Insert>,
    /// The indent `>` and Tab give (Vim's `shiftwidth`), in spaces.
    pub shiftwidth: usize,
    /// What the program keeps beside the text.
    pub note: N,
}

impl<N: Note> Doc<N> {
    pub fn new(text: Text) -> Doc<N> {
        Doc {
            text,
            row: 0,
            col: 0,
            insert: None,
            shiftwidth: 2,
            note: N::default(),
        }
    }

    pub fn line(&self, row: usize) -> String {
        self.text.line(row)
    }

    pub fn set_line(&mut self, row: usize, text: &str) {
        self.text.set_line(row, text);
    }

    /// Asks for Insert mode: at the cursor, or after the line's end (Vim's
    /// `startinsert` and `startinsert!`).
    pub fn start_insert(&mut self, at_end: bool) {
        self.insert = Some(if at_end { Insert::End } else { Insert::Here });
    }
}
