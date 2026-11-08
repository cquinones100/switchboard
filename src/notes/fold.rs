//! Collapsible sections, as Notes has them, ported from
//! notes/lua/notes/fold.lua: a title, heading or subheading folds everything
//! under it up to the next one of the same or a higher level. Enter or a
//! click on a heading's ▾ folds or unfolds it, and Vim's own z commands work
//! too. A folded heading shows as ▸ and its text, with how many lines are
//! hidden.
//!
//! Neovim worked the sections out from a fold expression and kept which
//! were closed itself; here the closed ones are kept by their heading's
//! row, and carried across an edit by the lines that didn't change
//! (`follow`).

use std::collections::BTreeSet;

use similar::{Algorithm, DiffTag, capture_diff_slices};

use super::render::Kind;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Folds {
    closed: BTreeSet<usize>,
}

/// The section a heading on `row` starts: its rows, the heading's first.
pub fn section(kinds: &[Kind], row: usize) -> Option<(usize, usize)> {
    let level = kinds.get(row)?.level()?;
    let end = (row + 1..kinds.len())
        .find(|&r| kinds[r].level().is_some_and(|l| l <= level))
        .map_or(kinds.len() - 1, |r| r - 1);
    Some((row, end))
}

impl Folds {
    /// The outermost closed section with `row` in it.
    pub fn closed_at(&self, kinds: &[Kind], row: usize) -> Option<(usize, usize)> {
        self.closed
            .iter()
            .filter_map(|&start| section(kinds, start))
            .find(|&(s, e)| row >= s && row <= e)
    }

    pub fn is_closed(&self, row: usize) -> bool {
        self.closed.contains(&row)
    }

    /// The rows shown: each closed section's heading stands for the section.
    pub fn visible(&self, kinds: &[Kind]) -> Vec<(usize, Option<usize>)> {
        let mut out = Vec::new();
        let mut row = 0;
        while row < kinds.len() {
            match self.closed_at(kinds, row) {
                Some((s, e)) if s == row => {
                    out.push((row, Some(e - s)));
                    row = e + 1;
                }
                _ => {
                    out.push((row, None));
                    row += 1;
                }
            }
        }
        out
    }

    /// Folds or unfolds what `row` is in, as Enter on a heading does: a
    /// closed section opens, a heading's section closes. Whether there was
    /// one.
    pub fn toggle(&mut self, kinds: &[Kind], row: usize) -> bool {
        if let Some((s, _)) = self.closed_at(kinds, row) {
            self.closed.remove(&s);
            return true;
        }
        if section(kinds, row).is_some() {
            self.closed.insert(row);
            return true;
        }
        false
    }

    /// `zc`: the innermost open section with `row` in it closes.
    pub fn close(&mut self, kinds: &[Kind], row: usize) {
        let inner = (0..=row.min(kinds.len().saturating_sub(1)))
            .rev()
            .filter(|&r| !self.closed.contains(&r))
            .find(|&r| section(kinds, r).is_some_and(|(_, e)| row <= e));
        if let Some(r) = inner {
            self.closed.insert(r);
        }
    }

    /// `zo`: the closed section with `row` in it opens.
    pub fn open(&mut self, kinds: &[Kind], row: usize) {
        if let Some((s, _)) = self.closed_at(kinds, row) {
            self.closed.remove(&s);
        }
    }

    /// `zM`: every section closed.
    pub fn close_all(&mut self, kinds: &[Kind]) {
        self.closed = (0..kinds.len())
            .filter(|&r| kinds[r].level().is_some())
            .collect();
    }

    /// `zR`: every section open.
    pub fn open_all(&mut self) {
        self.closed.clear();
    }

    /// The closed sections carried across an edit from `before` to `after`:
    /// a heading whose line didn't change keeps its fold where it went, one
    /// that changed or went loses it.
    pub fn follow(&mut self, before: &[String], after: &[String]) {
        if self.closed.is_empty() || before == after {
            return;
        }
        let mut moved = BTreeSet::new();
        for op in capture_diff_slices(Algorithm::Myers, before, after) {
            let (tag, old, new) = op.as_tag_tuple();
            if tag != DiffTag::Equal {
                continue;
            }
            for (o, n) in old.zip(new) {
                if self.closed.contains(&o) {
                    moved.insert(n);
                }
            }
        }
        self.closed = moved;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::render::kinds;

    #[test]
    fn a_heading_folds_its_section_and_a_fold_follows_its_heading() {
        let lines: Vec<String> = [
            "# Title", "intro", "## A", "a1", "## B", "b1", "### B.1", "b2",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let ks = kinds(&lines);
        assert_eq!(section(&ks, 2), Some((2, 3)));
        assert_eq!(section(&ks, 4), Some((4, 7)));
        assert_eq!(
            section(&ks, 0),
            Some((0, 7)),
            "the title folds the whole note"
        );
        let mut folds = Folds::default();
        assert!(folds.toggle(&ks, 2));
        assert_eq!(folds.visible(&ks)[2], (2, Some(1)));
        assert_eq!(folds.visible(&ks).len(), 7);
        let mut after = lines.clone();
        after.insert(1, "new".into());
        folds.follow(&lines, &after);
        assert!(folds.is_closed(3), "it moved down with its heading");
        let ks = kinds(&after);
        assert!(folds.toggle(&ks, 4), "a row in a closed section opens it");
        assert!(!folds.is_closed(3));
        assert!(!folds.toggle(&ks, 1), "body text isn't a heading");
    }
}
