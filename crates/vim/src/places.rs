//! Where a place in a text went after an edit, by a line diff: what is
//! kept by row (marks, folds, a jump list, the change list) moves with the
//! text through it. Split out of Notes' folds (crates/notes/src/fold.rs),
//! where it started, so every text the shared Vim reads follows its edits
//! the same way.

use similar::{Algorithm, DiffTag, capture_diff_slices};

/// Where each place `(row, byte)` of `before` went in `after`, by
/// `rows_after`: on its row, or, on a line joined onto the one before it
/// (`J`), at the same character of its text where the joined line has it,
/// as Vim moves a mark a join carries.
pub struct Places<'a> {
    before: &'a [String],
    after: &'a [String],
    rows: Vec<Option<usize>>,
}

impl<'a> Places<'a> {
    pub fn new(before: &'a [String], after: &'a [String]) -> Places<'a> {
        Places {
            before,
            after,
            rows: rows_after(before, after),
        }
    }

    pub fn of(&self, (r, c): (usize, usize)) -> Option<(usize, usize)> {
        let n = self.rows.get(r).copied().flatten()?;
        let start = self.before[r].len() - self.before[r].trim_start().len();
        Some(match self.joined_at(r, n) {
            Some(at) => (n, at + c.saturating_sub(start)),
            None if self.rows[..r].contains(&Some(n)) => {
                // The rest of a line a deletion reached into (`d` from the
                // line above into it): what is left of its end, at the same
                // character there, or where that starts for one deleted.
                let (was, now) = (&self.before[r], &self.after[n]);
                let kept: usize = was
                    .chars()
                    .rev()
                    .zip(now.chars().rev())
                    .take_while(|(a, b)| a == b)
                    .map(|(a, _)| a.len_utf8())
                    .sum();
                let (cut, from) = (was.len() - kept, now.len() - kept);
                (n, from + c.saturating_sub(cut))
            }
            None => (n, c),
        })
    }

    /// Where the text of a line joined onto the one before it starts in
    /// the joined line `n`, found after the text of the lines joined
    /// before it, in their order; none for a line not joined.
    fn joined_at(&self, r: usize, n: usize) -> Option<usize> {
        // The line above joined there, past any (a blank one) that went.
        let p = (0..r).rev().find(|&p| self.rows[p] == Some(n))?;
        let prev = &self.before[p];
        let from = match self.joined_at(p, n) {
            Some(at) => at + prev.trim_start().len(),
            None => prev.trim_end().len(),
        };
        let text = self.before[r].trim_start();
        let line = &self.after[n];
        if text.is_empty() || from > line.len() || !line.is_char_boundary(from) {
            return None;
        }
        line[from..].find(text).map(|i| from + i)
    }
}

/// Where each row of `before` went in `after`, by a line diff: an equal
/// line to its new row, a line in a run replaced by another to the line at
/// the same place in the new run (its last, past it), a deleted line
/// nowhere. The folds, the marks, the jump list and the change list follow
/// an edit through it, so what is kept by row moves with the text.
pub fn rows_after(before: &[String], after: &[String]) -> Vec<Option<usize>> {
    if before == after {
        return (0..before.len()).map(Some).collect();
    }
    let mut rows = vec![None; before.len()];
    // A checklist item, ticked or not, read the same.
    let item = |l: &str| l.replacen("[x]", "[ ]", 1).replacen("[X]", "[ ]", 1);
    // The same line, or the same item ticked or unticked.
    let same = |a: &str, b: &str| {
        a == b || {
            let it = item(a);
            it.contains("[ ]") && it == item(b)
        }
    };
    for op in capture_diff_slices(Algorithm::Myers, before, after) {
        let (tag, old, new) = op.as_tag_tuple();
        match tag {
            DiffTag::Equal => {
                for (o, n) in old.zip(new) {
                    rows[o] = Some(n);
                }
            }
            DiffTag::Replace if !new.is_empty() => {
                // Fewer lines than before: the run's first lines and its
                // last were joined into what is left, and a line between
                // goes unless its text is still there (`3J` joins it in),
                // as a deletion across lines removes it.
                // A line the run still has (an item ticked, a blank line put
                // in before it) goes to it first.
                let mut used = vec![false; new.len()];
                let mut found = vec![None; old.len()];
                for (k, o) in old.clone().enumerate() {
                    if before[o].trim().is_empty() {
                        continue;
                    }
                    if let Some(j) = (0..new.len())
                        .find(|&j| !used[j] && same(&before[o], &after[new.start + j]))
                    {
                        used[j] = true;
                        found[k] = Some(new.start + j);
                    }
                }
                let last = old.len() - 1;
                for (k, o) in old.clone().enumerate() {
                    rows[o] = if found[k].is_some() {
                        found[k]
                    } else if k == 0 || k + 1 < new.len() || old.len() <= new.len() {
                        Some(new.start + k)
                    } else if k == last {
                        // Joined when what is left ends as it did (`d`
                        // from one line into the next keeps the next's
                        // end); else replaced whole (`3cc`), and gone.
                        let text = before[o].trim_end();
                        let kept = after[new.end - 1].trim_end();
                        let tail = text.chars().count().min(1);
                        (tail > 0
                            && text
                                .chars()
                                .rev()
                                .take(tail)
                                .eq(kept.chars().rev().take(tail)))
                        .then_some(new.end - 1)
                    } else {
                        let text = before[o].trim();
                        new.clone()
                            .find(|&n| !text.is_empty() && after[n].contains(text))
                    };
                }
            }
            _ => {}
        }
    }
    // A line that moved (a checklist item ticked and sorted under the
    // ones left to do) goes where the same line, or the same item ticked or
    // unticked, was put in, rather than nowhere.
    let mut taken = vec![false; after.len()];
    for &n in rows.iter().flatten() {
        taken[n] = true;
    }
    for o in 0..before.len() {
        if rows[o].is_some() {
            continue;
        }
        let found = (0..after.len())
            .find(|&n| !taken[n] && after[n] == before[o])
            .or_else(|| {
                let it = item(&before[o]);
                (0..after.len()).find(|&n| {
                    !taken[n]
                        && after[n] != before[o]
                        && item(&after[n]) == it
                        && it.contains("[ ]")
                })
            });
        if let Some(n) = found {
            taken[n] = true;
            rows[o] = Some(n);
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_after_follows_equal_replaced_and_deleted_lines() {
        let lines = |s: &[&str]| -> Vec<String> { s.iter().map(|s| s.to_string()).collect() };
        let before = lines(&["a", "b", "c", "d", "e"]);
        let after = lines(&["new", "a", "B", "d"]);
        let rows = rows_after(&before, &after);
        assert_eq!(rows[0], Some(1), "an equal line to its new row");
        assert_eq!(rows[1], Some(2), "a replaced line to its replacement");
        assert_eq!(rows[2], None, "the run's last line, replaced, goes");
        assert_eq!(rows[3], Some(3));
        assert_eq!(rows[4], None, "a deleted line nowhere");
        let rows = rows_after(
            &lines(&["x", "AAAAA", "BBBBB", "CCCCC", "y"]),
            &lines(&["x", "ACCCC", "y"]),
        );
        assert_eq!(
            rows[1..4],
            [Some(1), None, Some(1)],
            "a line deleted between goes"
        );
        let rows = rows_after(
            &lines(&["alpha", "beta", "gamma"]),
            &lines(&["alpha beta gamma"]),
        );
        assert_eq!(rows, [Some(0), Some(0), Some(0)], "a line joined in stays");
        let rows = rows_after(
            &lines(&["# T", "alpha", "beta", "gamma", "tail"]),
            &lines(&["# T", "new", "tail"]),
        );
        assert_eq!(rows[3], None, "a line replaced whole goes");
        let rows = rows_after(&lines(&["first", "second", "third"]), &lines(&["fd"]));
        assert_eq!(rows[2], Some(0), "its last character left, joined");
        let rows = rows_after(
            &lines(&["alpha", "beta", "x"]),
            &lines(&["ALPHA", "BETA", "x"]),
        );
        assert_eq!(
            rows[..2],
            [Some(0), Some(1)],
            "as many lines changed, each keeps its row"
        );
        let before = lines(&["- [ ] alpha", "- [ ] beta", "- [ ] gamma"]);
        let after = lines(&["- [ ] beta", "- [ ] gamma", "- [x] alpha"]);
        assert_eq!(
            rows_after(&before, &after)[0],
            Some(2),
            "an item ticked and sorted down goes with it"
        );
        let rows = rows_after(
            &lines(&["- [ ] a", "- [ ] b"]),
            &lines(&["- [ ] a", "", "- [x] b"]),
        );
        assert_eq!(rows[1], Some(2), "past the blank line put in before it");
    }
}
