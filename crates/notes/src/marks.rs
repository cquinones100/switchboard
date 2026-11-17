//! A note's marks, as Neovim keeps a buffer's: `m{a-z}` sets one at the
//! cursor, `` `{a-z} `` goes back to it and `'{a-z}` to its line, and the
//! context mark, where the cursor last jumped from, is `` `` `` and `''`.
//! New here, since the plugin left marks to Neovim.
//!
//! A mark is kept by row and column and carried across every edit by the
//! line diff the folds use (`fold::rows_after`), so it stays on its line
//! as lines come and go above it; a mark whose line is deleted goes, as
//! Neovim drops one. The editor keeps them per note while the app runs.

use std::collections::BTreeMap;

use super::fold::Places;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    named: BTreeMap<char, (usize, usize)>,
    /// When each mark was set, counted in `sets`, so undo can tell a mark
    /// set again since from one only an edit moved.
    set_as: BTreeMap<char, u64>,
    sets: u64,
    /// Where the cursor last jumped from, and when it was set.
    pub context: Option<(usize, usize)>,
    context_set_as: u64,
}

impl Marks {
    pub fn set(&mut self, name: char, at: (usize, usize)) {
        self.named.insert(name, at);
        self.sets += 1;
        self.set_as.insert(name, self.sets);
    }

    pub fn set_context(&mut self, at: (usize, usize)) {
        self.context = Some(at);
        self.sets += 1;
        self.context_set_as = self.sets;
    }

    pub fn get(&self, name: char) -> Option<(usize, usize)> {
        self.named.get(&name).copied()
    }

    /// The named marks as `then` had them, for undo putting back the text
    /// they were set in: one a change took, or moved by joining its line
    /// to another, back where it was; one set since stays.
    pub fn restore(&mut self, then: &Marks) {
        for (&name, &at) in &then.named {
            if self.set_as.get(&name) == then.set_as.get(&name) {
                self.named.insert(name, at);
            }
        }
        if self.context_set_as == then.context_set_as {
            self.context = then.context;
        }
    }

    /// The marks carried across an edit from `before` to `after`.
    pub fn follow(&mut self, before: &[String], after: &[String]) {
        if before == after {
            return;
        }
        let places = Places::new(before, after);
        let moved = |at: (usize, usize)| places.of(at);
        self.named = std::mem::take(&mut self.named)
            .into_iter()
            .filter_map(|(name, at)| moved(at).map(|at| (name, at)))
            .collect();
        self.context = self.context.and_then(moved);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_mark_moves_with_its_line_and_goes_with_it() {
        let before = lines(&["a", "b", "c"]);
        let mut marks = Marks::default();
        marks.set('a', (1, 1));
        marks.set('b', (2, 0));
        marks.set_context((0, 0));
        marks.follow(&before, &lines(&["new", "a", "b", "c"]));
        assert_eq!(marks.get('a'), Some((2, 1)), "down with a line put above");
        assert_eq!(marks.context, Some((1, 0)));
        marks.follow(
            &lines(&["new", "a", "b", "c"]),
            &lines(&["new", "a", "B", "c"]),
        );
        assert_eq!(marks.get('a'), Some((2, 1)), "a changed line keeps it");
        marks.follow(&lines(&["new", "a", "B", "c"]), &lines(&["new", "a", "B"]));
        assert_eq!(marks.get('b'), None, "its line deleted, it goes");
        assert_eq!(marks.get('a'), Some((2, 1)));
        let mut marks = Marks::default();
        marks.set('a', (2, 2));
        marks.follow(
            &lines(&["# T", "one", "  two"]),
            &lines(&["# T", "one two"]),
        );
        assert_eq!(
            marks.get('a'),
            Some((1, 4)),
            "joined on, at the same character"
        );
        let mut marks = Marks::default();
        marks.set('a', (1, 0));
        marks.follow(&lines(&["one", "the", "other"]), &lines(&["one the other"]));
        assert_eq!(marks.get('a'), Some((0, 4)), "found in the order joined");
        let mut marks = Marks::default();
        marks.set('a', (1, 3));
        marks.follow(&lines(&["one", "abcdef"]), &lines(&["odef"]));
        assert_eq!(
            marks.get('a'),
            Some((0, 1)),
            "on what is left of a line deleted into"
        );
        let mut marks = Marks::default();
        marks.set('a', (2, 4));
        marks.follow(&lines(&["abc", "middle", "uvwxyz"]), &lines(&["axyz"]));
        assert_eq!(marks.get('a'), Some((0, 2)), "past lines the deletion took");
        let mut marks = Marks::default();
        marks.set('a', (2, 0));
        marks.follow(
            &lines(&["alpha", "", "beta", "end"]),
            &lines(&["alpha beta end"]),
        );
        assert_eq!(marks.get('a'), Some((0, 6)), "joined past a blank line");
        let mut marks = Marks::default();
        marks.set('a', (2, 4));
        marks.follow(&lines(&["abc", "middle", "uvwxyz"]), &lines(&["axyz"]));
        assert_eq!(marks.get('a'), Some((0, 2)), "past lines the deletion took");
    }
}
