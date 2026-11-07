//! A note's text as the app edits it: a list of lines, as a Neovim buffer
//! held them for the plugin (`nvim_buf_get_lines` and `nvim_buf_set_lines`
//! in every file of notes/lua/notes/), kept in a rope.
//!
//! The rope holds the lines joined by newlines with none after the last, so
//! there is always at least one line, an empty note being one empty line, as
//! Neovim has it; the file is that and a newline. A rope's clone shares its
//! pieces with the original, so the undo history can keep the whole text at
//! every step for little, and an edit in a long note touches only the
//! pieces around it.
//!
//! Lines are split on `\n` alone: a carriage return or a form feed in a note
//! is text, as it was to Neovim reading a file with Unix line endings.

use ropey::Rope;

#[derive(Clone, Debug, Default)]
pub struct Text {
    rope: Rope,
}

impl PartialEq for Text {
    fn eq(&self, other: &Self) -> bool {
        self.rope == other.rope
    }
}

impl Text {
    /// A note's text from its file: the newline that ends the file isn't a
    /// line of its own.
    pub fn from_file(text: &str) -> Text {
        let body = text.strip_suffix('\n').unwrap_or(text);
        Text {
            rope: Rope::from_str(body),
        }
    }

    pub fn from_lines<S: AsRef<str>>(lines: &[S]) -> Text {
        let joined: Vec<&str> = lines.iter().map(|l| l.as_ref()).collect();
        Text {
            rope: Rope::from_str(&joined.join("\n")),
        }
    }

    /// The text as its file holds it: the lines, each ending in a newline.
    pub fn file_text(&self) -> String {
        let mut text = self.rope.to_string();
        text.push('\n');
        text
    }

    /// How many lines there are, never fewer than one.
    pub fn len(&self) -> usize {
        self.rope.len_lines()
    }

    /// Always false: a note has at least its one line.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// A line, 0-based, without its newline; empty past the end.
    pub fn line(&self, row: usize) -> String {
        if row >= self.len() {
            return String::new();
        }
        let mut line = self.rope.line(row).to_string();
        if line.ends_with('\n') {
            line.pop();
        }
        line
    }

    /// Every line.
    pub fn lines(&self) -> Vec<String> {
        (0..self.len()).map(|r| self.line(r)).collect()
    }

    /// Replaces lines `start..end` (0-based, `end` exclusive) with `new`, as
    /// `nvim_buf_set_lines` does: `start == end` puts the new lines before
    /// line `start` (after the last when it is the line count), and no new
    /// lines deletes the range. The note never ends up with no line.
    pub fn set_lines<S: AsRef<str>>(&mut self, start: usize, end: usize, new: &[S]) {
        let len = self.len();
        let start = start.min(len);
        let end = end.clamp(start, len);
        let joined = new
            .iter()
            .map(|l| l.as_ref())
            .collect::<Vec<_>>()
            .join("\n");
        let chars = self.rope.len_chars();
        let at = |row: usize| {
            if row >= len {
                chars
            } else {
                self.rope.line_to_char(row)
            }
        };
        if start == end {
            if new.is_empty() {
                return;
            }
            if start < len {
                self.rope.insert(at(start), &format!("{joined}\n"));
            } else {
                self.rope.insert(chars, &format!("\n{joined}"));
            }
            return;
        }
        let (from, to) = (at(start), at(end));
        if !new.is_empty() {
            self.rope.remove(from..to);
            let tail = if end < len { "\n" } else { "" };
            self.rope.insert(from, &format!("{joined}{tail}"));
            return;
        }
        if end < len {
            self.rope.remove(from..to);
        } else if start > 0 {
            // The last lines go, and the newline before them with them.
            self.rope.remove(from - 1..chars);
        } else {
            self.rope = Rope::new();
        }
    }

    /// Replaces one line.
    pub fn set_line(&mut self, row: usize, text: &str) {
        self.set_lines(row, row + 1, &[text]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_set_as_neovim_sets_them() {
        let mut t = Text::from_file("a\nb\nc\n");
        assert_eq!(t.lines(), ["a", "b", "c"]);
        t.set_lines(1, 2, &["B", "B2"]);
        assert_eq!(t.lines(), ["a", "B", "B2", "c"]);
        t.set_lines(0, 0, &["top"]);
        assert_eq!(t.lines(), ["top", "a", "B", "B2", "c"]);
        t.set_lines(5, 5, &["end"]);
        assert_eq!(t.lines(), ["top", "a", "B", "B2", "c", "end"]);
        t.set_lines(4, 6, &[] as &[&str]);
        assert_eq!(t.lines(), ["top", "a", "B", "B2"]);
        t.set_lines(1, 2, &[] as &[&str]);
        assert_eq!(t.lines(), ["top", "B", "B2"]);
        t.set_lines(0, 3, &[] as &[&str]);
        assert_eq!(t.lines(), [""]);
        assert_eq!(t.file_text(), "\n");
        t.set_line(0, "only");
        assert_eq!(t.file_text(), "only\n");
    }

    #[test]
    fn a_carriage_return_is_text() {
        let t = Text::from_file("a\r\nb\n");
        assert_eq!(t.lines(), ["a\r", "b"]);
    }
}
