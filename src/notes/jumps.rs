//! The jump list, as Neovim keeps one: every place the cursor jumped from
//! (a search, `G` and `gg`, `%`, `{` and `}`, `H` `M` `L`, a mark, a link
//! followed, a note opened from another), which Ctrl-O goes back through
//! and Ctrl-I (or Tab) forward. New here, since the plugin left it to
//! Neovim.
//!
//! One list for the app, across notes, as Neovim's spans buffers, kept in
//! the notes directory's `.notes.json` (`saved`, `from_saved`), as Neovim's
//! shada keeps it, since Notes ends and starts again as it moves between
//! tabs: an entry names its note, and each note's entries
//! follow its edits by the line diff the folds use (`fold::rows_after`),
//! an entry whose line went dropped, as Neovim drops one. Going back from
//! the newest records where the cursor is first, so Ctrl-I comes back.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::fold::Places;

/// As many as Neovim keeps.
const MOST: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Jump {
    pub note: PathBuf,
    pub row: usize,
    pub col: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Jumps {
    list: Vec<Jump>,
    /// The entry Ctrl-O and Ctrl-I last went to; the list's length past
    /// the newest.
    at: usize,
    /// The entry at `at` went with its line, so `at` is the one after it,
    /// which Ctrl-I goes to next.
    between: bool,
}

impl Jumps {
    /// A place jumped from: an earlier entry on its line goes, and the
    /// list starts again from the newest.
    pub fn record(&mut self, jump: Jump) {
        self.list
            .retain(|j| !(j.note == jump.note && j.row == jump.row));
        self.list.push(jump);
        if self.list.len() > MOST {
            self.list.remove(0);
        }
        self.at = self.list.len();
        self.between = false;
    }

    /// Ctrl-O: `count` entries back, stopping at the oldest, passing over
    /// one whose note is gone (`exists`), which goes; from the newest,
    /// `now` is recorded first.
    pub fn back(
        &mut self,
        count: usize,
        now: Jump,
        exists: impl Fn(&Path) -> bool,
    ) -> Option<Jump> {
        if self.at >= self.list.len() {
            self.record(now);
            self.at = self.list.len() - 1;
        }
        self.drop_gone(&exists);
        if self.at == 0 || self.list.is_empty() {
            return None;
        }
        self.at -= count.clamp(1, self.at);
        self.between = false;
        self.list.get(self.at).cloned()
    }

    /// Ctrl-I: `count` entries forward, passing over one whose note is
    /// gone; nothing past the newest.
    pub fn forward(&mut self, count: usize, exists: impl Fn(&Path) -> bool) -> Option<Jump> {
        self.drop_gone(&exists);
        let next = self.at + 1 - usize::from(self.between);
        if next >= self.list.len() {
            return None;
        }
        // A count past the newest stops at it, as one past the oldest does.
        let to = (self.at.saturating_add(count.max(1)) - usize::from(self.between))
            .min(self.list.len() - 1);
        self.at = to;
        self.between = false;
        self.list.get(to).cloned()
    }

    /// The list as `.notes.json` keeps it, each note named by `rel`, oldest
    /// first.
    pub fn saved(&self, rel: impl Fn(&Path) -> String) -> Value {
        self.list
            .iter()
            .map(|j| json!({"note": rel(&j.note), "row": j.row, "col": j.col}))
            .collect()
    }

    /// The list `saved` kept, each note's path from `absolute`, an entry
    /// whose note is gone (`exists`) left out, Ctrl-O starting from the
    /// newest.
    pub fn from_saved(
        saved: &Value,
        absolute: impl Fn(&str) -> PathBuf,
        exists: impl Fn(&Path) -> bool,
    ) -> Jumps {
        let mut jumps = Jumps::default();
        for entry in saved.as_array().into_iter().flatten() {
            let field = |k: &str| entry.get(k).and_then(Value::as_u64).map(|n| n as usize);
            let (Some(note), Some(row), Some(col)) = (
                entry.get("note").and_then(Value::as_str),
                field("row"),
                field("col"),
            ) else {
                continue;
            };
            let note = absolute(note);
            if exists(&note) {
                jumps.record(Jump { note, row, col });
            }
        }
        jumps
    }

    /// Whether the entries are those of `other`, wherever each list is.
    pub fn same_entries(&self, other: &Jumps) -> bool {
        self.list == other.list
    }

    /// The entry Ctrl-O and Ctrl-I last went to.
    pub fn current(&self) -> Option<&Jump> {
        self.list.get(self.at)
    }

    /// The entries whose note is gone, gone too.
    fn drop_gone(&mut self, exists: &impl Fn(&Path) -> bool) {
        let keep: Vec<bool> = self.list.iter().map(|j| exists(&j.note)).collect();
        self.retain(|i, _| keep[i]);
    }

    /// The entries kept, `at` staying on the same one, or, when it went,
    /// between the ones either side of it.
    fn retain(&mut self, mut keep: impl FnMut(usize, &mut Jump) -> bool) {
        let mut kept = Vec::with_capacity(self.list.len());
        let mut at = None;
        for (i, mut j) in std::mem::take(&mut self.list).into_iter().enumerate() {
            if i == self.at {
                at = Some(kept.len());
            }
            if keep(i, &mut j) {
                kept.push(j);
            } else if i == self.at {
                self.between = true;
            }
        }
        self.at = at.unwrap_or(kept.len());
        self.list = kept;
    }

    /// A note's entries carried across an edit from `before` to `after`.
    pub fn follow(&mut self, note: &Path, before: &[String], after: &[String]) {
        if before == after {
            return;
        }
        let places = Places::new(before, after);
        self.retain(|_, j| {
            if j.note != note {
                return true;
            }
            match places.of((j.row, j.col)) {
                Some((r, c)) => {
                    (j.row, j.col) = (r, c);
                    true
                }
                None => false,
            }
        });
    }

    /// Notes moved or renamed: `to` gives a note's new path, or nothing
    /// for one that stayed.
    pub fn rekey(&mut self, to: &dyn Fn(&Path) -> Option<PathBuf>) {
        for j in &mut self.list {
            if let Some(n) = to(&j.note) {
                j.note = n;
            }
        }
    }

    /// A note gone: its entries go too.
    pub fn forget(&mut self, note: &Path) {
        self.retain(|_, j| j.note != note);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(note: &str, row: usize) -> Jump {
        Jump {
            note: PathBuf::from(note),
            row,
            col: 0,
        }
    }

    fn all(_: &Path) -> bool {
        true
    }

    #[test]
    fn a_jump_on_a_line_already_listed_takes_its_place() {
        let mut jumps = Jumps::default();
        jumps.record(at("a", 1));
        jumps.record(at("a", 5));
        jumps.record(at("b", 1));
        jumps.record(at("a", 1));
        assert_eq!(jumps.list, vec![at("a", 5), at("b", 1), at("a", 1)]);
    }

    #[test]
    fn back_from_the_newest_records_where_it_is_and_forward_comes_back() {
        let mut jumps = Jumps::default();
        jumps.record(at("a", 1));
        jumps.record(at("a", 5));
        assert_eq!(jumps.back(1, at("a", 9), all), Some(at("a", 5)));
        assert_eq!(
            jumps.back(5, at("a", 0), all),
            Some(at("a", 1)),
            "stops at the oldest"
        );
        assert_eq!(jumps.back(1, at("a", 0), all), None);
        assert_eq!(jumps.forward(2, all), Some(at("a", 9)), "back where it was");
        assert_eq!(jumps.forward(1, all), None, "nothing past the newest");
        jumps.back(2, at("a", 0), all);
        assert_eq!(
            jumps.forward(9, all),
            Some(at("a", 9)),
            "a count past it stops there"
        );
    }

    #[test]
    fn a_note_gone_is_passed_over() {
        let mut jumps = Jumps::default();
        jumps.record(at("a", 1));
        jumps.record(at("gone", 1));
        let exists = |p: &Path| p != Path::new("gone");
        assert_eq!(jumps.back(1, at("a", 9), exists), Some(at("a", 1)));
        assert_eq!(jumps.forward(1, exists), Some(at("a", 9)));
    }

    #[test]
    fn the_list_saved_comes_back_without_a_gone_note_and_starts_at_the_newest() {
        let mut jumps = Jumps::default();
        jumps.record(at("/n/a.md", 3));
        jumps.record(at("/n/gone.md", 1));
        jumps.record(at("/n/b.md", 7));
        let saved = jumps.saved(|p| p.strip_prefix("/n/").unwrap().display().to_string());
        assert_eq!(saved[0], json!({"note": "a.md", "row": 3, "col": 0}));
        let mut back = Jumps::from_saved(
            &saved,
            |rel| Path::new("/n").join(rel),
            |p| p != Path::new("/n/gone.md"),
        );
        assert_eq!(back.list, vec![at("/n/a.md", 3), at("/n/b.md", 7)]);
        assert_eq!(back.back(1, at("/n/c.md", 0), all), Some(at("/n/b.md", 7)));
    }

    #[test]
    fn a_saved_list_that_isnt_one_comes_back_empty() {
        for saved in [json!(null), json!("x"), json!([{"note": 1}, {"row": 2}])] {
            let jumps = Jumps::from_saved(&saved, |s: &str| PathBuf::from(s), all);
            assert!(jumps.list.is_empty(), "{saved}");
        }
    }

    #[test]
    fn it_keeps_a_hundred() {
        let mut jumps = Jumps::default();
        for row in 0..150 {
            jumps.record(at("a", row));
        }
        assert_eq!(jumps.list.len(), 100);
        assert_eq!(jumps.list[0], at("a", 50));
    }

    #[test]
    fn an_entry_follows_its_line_and_goes_with_it() {
        let lines = |s: &[&str]| -> Vec<String> { s.iter().map(|s| s.to_string()).collect() };
        let mut jumps = Jumps::default();
        jumps.record(at("a", 1));
        jumps.record(at("a", 2));
        jumps.record(at("b", 1));
        jumps.follow(
            Path::new("a"),
            &lines(&["x", "y", "z"]),
            &lines(&["new", "x", "z"]),
        );
        assert_eq!(jumps.list, vec![at("a", 2), at("b", 1)]);
    }

    #[test]
    fn the_entry_gone_to_going_leaves_ctrl_i_on_the_one_before_it() {
        let lines = |s: &[&str]| -> Vec<String> { s.iter().map(|s| s.to_string()).collect() };
        let mut jumps = Jumps::default();
        jumps.record(at("a", 0));
        jumps.record(at("a", 1));
        assert_eq!(jumps.back(1, at("a", 2), all), Some(at("a", 1)));
        jumps.follow(
            Path::new("a"),
            &lines(&["x", "y", "z"]),
            &lines(&["x", "z"]),
        );
        assert_eq!(
            jumps.forward(1, all),
            Some(at("a", 1)),
            "on to the one after it"
        );
    }

    #[test]
    fn the_oldest_entry_going_leaves_ctrl_i_before_the_next() {
        let lines = |s: &[&str]| -> Vec<String> { s.iter().map(|s| s.to_string()).collect() };
        let mut jumps = Jumps::default();
        jumps.record(at("a", 0));
        jumps.record(at("a", 1));
        assert_eq!(jumps.back(2, at("a", 2), all), Some(at("a", 0)));
        jumps.follow(
            Path::new("a"),
            &lines(&["x", "y", "z"]),
            &lines(&["y", "z"]),
        );
        assert_eq!(
            jumps.forward(1, all),
            Some(at("a", 0)),
            "the next, moved up"
        );
        assert_eq!(jumps.forward(1, all), Some(at("a", 1)));
    }
}
