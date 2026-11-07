//! One file's change laid out side by side, as M-d's diffview showed it in
//! Vim's diff mode: the old text on the left and the new on the right, each
//! line beside the one it became, a line only one side has facing a blank
//! gap (not hatched), and unchanged runs more than `CONTEXT` lines from a
//! change folded away.
//!
//! Lines are matched as people read them, as diffview's `diffopt` asked:
//! the histogram algorithm, and within a changed block each old line paired
//! with the new line most like it (`linematch`), so fewer lines look changed
//! than were. Within a changed line, the words that changed are marked
//! (`inline:word`), not the span from the first difference to the last,
//! which in a paragraph of prose on one line is most of it.
//!
//! Each side's lines are then wrapped to its width (`wrap`), a line long
//! enough to wrap taking as many rows as its longer side needs, so a
//! paragraph of Markdown on one line reads whole.

use std::collections::HashSet;
use std::ops::Range;

use similar::{Algorithm, DiffTag, capture_diff_slices};
use unicode_width::UnicodeWidthChar;

/// Unchanged lines kept beside a change, as diffview's `context:3`.
pub const CONTEXT: usize = 3;
/// The largest changed block whose lines are paired by likeness; a larger
/// one is paired in order, as Vim's `linematch:60` gave up past its limit.
const PAIR_LIMIT: usize = 900;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The same on both sides.
    Same,
    /// On both sides, different.
    Changed,
    /// Only on the new side.
    Added,
    /// Only on the old side.
    Removed,
}

/// One row of the two sides: the old line and the new beside it (indexes
/// from 0), and the words that changed in each, as byte ranges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub kind: Kind,
    pub old_words: Vec<Range<usize>>,
    pub new_words: Vec<Range<usize>>,
}

impl Row {
    fn same(old: usize, new: usize) -> Row {
        Row {
            old: Some(old),
            new: Some(new),
            kind: Kind::Same,
            old_words: Vec::new(),
            new_words: Vec::new(),
        }
    }

    pub fn changes(&self) -> bool {
        self.kind != Kind::Same
    }
}

/// A line's tokens for comparing words: runs of letters, digits and _, runs
/// of whitespace, and each other character alone.
fn tokens(line: &str) -> Vec<Range<usize>> {
    #[derive(PartialEq)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut last: Option<Class> = None;
    for (i, c) in line.char_indices() {
        let now = class(c);
        let joins = matches!(
            (&last, &now),
            (Some(Class::Word), Class::Word) | (Some(Class::Space), Class::Space)
        );
        match out.last_mut() {
            Some(r) if joins => r.end = i + c.len_utf8(),
            _ => out.push(i..i + c.len_utf8()),
        }
        last = Some(now);
    }
    out
}

/// The words that changed between two lines, as byte ranges in each, with
/// the space between two changed words marked too, so a changed phrase
/// reads as one.
pub fn changed_words(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let a = tokens(old);
    let b = tokens(new);
    let at: Vec<&str> = a.iter().map(|r| &old[r.clone()]).collect();
    let bt: Vec<&str> = b.iter().map(|r| &new[r.clone()]).collect();
    let mut old_changed = vec![false; a.len()];
    let mut new_changed = vec![false; b.len()];
    for op in capture_diff_slices(Algorithm::Myers, &at, &bt) {
        let (tag, o, n) = op.as_tag_tuple();
        if tag != DiffTag::Equal {
            o.for_each(|i| old_changed[i] = true);
            n.for_each(|i| new_changed[i] = true);
        }
    }
    (
        spans(old, &a, &mut old_changed),
        spans(new, &b, &mut new_changed),
    )
}

fn spans(line: &str, tokens: &[Range<usize>], changed: &mut [bool]) -> Vec<Range<usize>> {
    for i in 1..tokens.len().saturating_sub(1) {
        if !changed[i]
            && changed[i - 1]
            && changed[i + 1]
            && line[tokens[i].clone()].chars().all(char::is_whitespace)
        {
            changed[i] = true;
        }
    }
    let mut out: Vec<Range<usize>> = Vec::new();
    for (i, r) in tokens.iter().enumerate() {
        if !changed[i] {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.end == r.start => last.end = r.end,
            _ => out.push(r.clone()),
        }
    }
    out
}

/// How alike two lines are, from 0 to 1: the share of their words they
/// have in common.
fn likeness(a: &str, b: &str) -> f64 {
    let at: Vec<&str> = tokens(a).into_iter().map(|r| &a[r]).collect();
    let bt: Vec<&str> = tokens(b).into_iter().map(|r| &b[r]).collect();
    if at.is_empty() && bt.is_empty() {
        return 1.0;
    }
    let same: usize = capture_diff_slices(Algorithm::Myers, &at, &bt)
        .iter()
        .filter(|op| op.tag() == DiffTag::Equal)
        .map(|op| op.old_range().len())
        .sum();
    2.0 * same as f64 / (at.len() + bt.len()) as f64
}

/// A changed block's rows: its old and new lines side by side, in order,
/// as many pairs as the shorter side has lines, the longer side's extra
/// lines facing a gap where they are least like what the other side has.
fn pair(old: &[String], new: &[String], o: Range<usize>, n: Range<usize>, rows: &mut Vec<Row>) {
    let (h, w) = (o.len(), n.len());
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    if h != w && h * w <= PAIR_LIMIT {
        // As many pairs as the shorter side has lines, chosen so the pairs
        // are as alike as they can be: the longer side's lines left over
        // are the ones least like anything facing them.
        let like: Vec<Vec<f64>> = o
            .clone()
            .map(|i| n.clone().map(|j| likeness(&old[i], &new[j])).collect())
            .collect();
        let mut best = vec![vec![0.0f64; w + 1]; h + 1];
        for i in (0..h).rev() {
            for j in (0..w).rev() {
                let mut b = like[i][j] + best[i + 1][j + 1];
                if h - i > w - j {
                    b = b.max(best[i + 1][j]);
                }
                if w - j > h - i {
                    b = b.max(best[i][j + 1]);
                }
                best[i][j] = b;
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < h && j < w {
            if best[i][j] == like[i][j] + best[i + 1][j + 1] {
                pairs.push((i, j));
                i += 1;
                j += 1;
            } else if h - i > w - j {
                i += 1;
            } else {
                j += 1;
            }
        }
    } else {
        pairs = (0..h.min(w)).map(|k| (k, k)).collect();
    }
    let (mut i, mut j) = (0, 0);
    let push_until = |pi: usize, pj: usize, rows: &mut Vec<Row>, i: &mut usize, j: &mut usize| {
        while *i < pi {
            rows.push(Row {
                old: Some(o.start + *i),
                new: None,
                kind: Kind::Removed,
                old_words: Vec::new(),
                new_words: Vec::new(),
            });
            *i += 1;
        }
        while *j < pj {
            rows.push(Row {
                old: None,
                new: Some(n.start + *j),
                kind: Kind::Added,
                old_words: Vec::new(),
                new_words: Vec::new(),
            });
            *j += 1;
        }
    };
    for (pi, pj) in pairs {
        push_until(pi, pj, rows, &mut i, &mut j);
        let (oi, nj) = (o.start + pi, n.start + pj);
        let (old_words, new_words) = changed_words(&old[oi], &new[nj]);
        rows.push(Row {
            old: Some(oi),
            new: Some(nj),
            kind: Kind::Changed,
            old_words,
            new_words,
        });
        i = pi + 1;
        j = pj + 1;
    }
    push_until(h, w, rows, &mut i, &mut j);
}

/// The two sides' lines as rows.
pub fn align(old: &[String], new: &[String]) -> Vec<Row> {
    let a: Vec<&str> = old.iter().map(String::as_str).collect();
    let b: Vec<&str> = new.iter().map(String::as_str).collect();
    let mut rows = Vec::new();
    // A deletion beside an insertion is one changed block, whichever way
    // the algorithm reports it.
    let mut pending: Option<(Range<usize>, Range<usize>)> = None;
    let flush = |pending: &mut Option<(Range<usize>, Range<usize>)>, rows: &mut Vec<Row>| {
        if let Some((o, n)) = pending.take() {
            pair(old, new, o, n, rows);
        }
    };
    for op in capture_diff_slices(Algorithm::Histogram, &a, &b) {
        let (tag, o, n) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            flush(&mut pending, &mut rows);
            for k in 0..o.len() {
                rows.push(Row::same(o.start + k, n.start + k));
            }
            continue;
        }
        pending = Some(match pending.take() {
            Some((po, pn)) => (po.start..o.end.max(po.end), pn.start..n.end.max(pn.end)),
            None => (o, n),
        });
    }
    flush(&mut pending, &mut rows);
    rows
}

/// The rows of a file shown alone on one side (added, or deleted as it
/// was): every line, unmarked.
pub fn alone(lines: &[String], new_side: bool) -> Vec<Row> {
    (0..lines.len())
        .map(|i| Row {
            old: (!new_side).then_some(i),
            new: new_side.then_some(i),
            kind: Kind::Same,
            old_words: Vec::new(),
            new_words: Vec::new(),
        })
        .collect()
}

/// What is shown of the rows: each row, or a fold standing for a run of
/// unchanged ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Row(usize),
    Fold { first: usize, count: usize },
}

/// The rows as shown: unchanged runs more than `CONTEXT` rows from any
/// change folded, unless opened (by the fold's first row), or `all` open.
/// A file with no change at all is one fold.
pub fn items(rows: &[Row], opened: &HashSet<usize>, all: bool) -> Vec<Item> {
    let mut near = vec![all; rows.len()];
    if !all {
        for (i, row) in rows.iter().enumerate() {
            if row.changes() {
                let from = i.saturating_sub(CONTEXT);
                let to = (i + CONTEXT + 1).min(rows.len());
                near[from..to].iter_mut().for_each(|n| *n = true);
            }
        }
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        if near[i] {
            out.push(Item::Row(i));
            i += 1;
            continue;
        }
        let first = i;
        while i < rows.len() && !near[i] {
            i += 1;
        }
        let count = i - first;
        if count < 2 || opened.contains(&first) {
            out.extend((first..i).map(Item::Row));
        } else {
            out.push(Item::Fold { first, count });
        }
    }
    out
}

/// The columns a character takes.
pub fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// A line broken into pieces at most `cols` wide, as byte ranges: at the
/// last space that lets a piece fit where there is one, else where it
/// fills. An empty line is one empty piece.
pub fn wrap(line: &str, cols: usize) -> Vec<Range<usize>> {
    let cols = cols.max(1);
    let mut out = Vec::new();
    let mut start = 0;
    while start < line.len() {
        let mut used = 0;
        let mut end = start;
        let mut space: Option<usize> = None;
        for (i, c) in line[start..].char_indices() {
            let w = char_width(c);
            if used + w > cols {
                break;
            }
            used += w;
            end = start + i + c.len_utf8();
            if c == ' ' {
                space = Some(end);
            }
        }
        if end == start {
            // A character wider than the piece: it goes alone.
            let c = line[start..].chars().next().expect("not at the end");
            end = start + c.len_utf8();
        } else if end < line.len()
            && let Some(at) = space.filter(|at| *at > start)
        {
            end = at;
        }
        out.push(start..end);
        start = end;
    }
    if out.is_empty() {
        out.push(0..0);
    }
    out
}

/// A file's text as lines for the screen: tabs as four spaces, carriage
/// returns dropped, other control characters shown as ·.
pub fn lines_of(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|l| {
            let l = l.strip_suffix('\r').unwrap_or(l);
            let mut out = String::with_capacity(l.len());
            for c in l.chars() {
                match c {
                    '\t' => out.push_str("    "),
                    c if c.is_control() => out.push('·'),
                    c => out.push(c),
                }
            }
            out
        })
        .collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        lines_of(text)
    }

    #[test]
    fn a_small_diff_lines_up_each_line_beside_what_it_became() {
        let old = lines("one\ntwo\nthree\nfour\n");
        let new = lines("one\ntoo\nthree\nthree and a half\nfour\n");
        let rows = align(&old, &new);
        let shape: Vec<(Option<usize>, Option<usize>, Kind)> =
            rows.iter().map(|r| (r.old, r.new, r.kind)).collect();
        assert_eq!(
            shape,
            vec![
                (Some(0), Some(0), Kind::Same),
                (Some(1), Some(1), Kind::Changed),
                (Some(2), Some(2), Kind::Same),
                (None, Some(3), Kind::Added),
                (Some(3), Some(4), Kind::Same),
            ]
        );
        assert_eq!(rows[1].old_words, vec![0..3]);
        assert_eq!(rows[1].new_words, vec![0..3]);
    }

    #[test]
    fn a_changed_block_pairs_each_line_with_the_one_most_like_it() {
        let old = lines("let a = 1;\nlet b = 2;\n");
        let new = lines("// new comment\nlet a = 10;\nlet b = 2 + x;\n");
        let rows = align(&old, &new);
        let shape: Vec<(Option<usize>, Option<usize>, Kind)> =
            rows.iter().map(|r| (r.old, r.new, r.kind)).collect();
        assert_eq!(
            shape,
            vec![
                (None, Some(0), Kind::Added),
                (Some(0), Some(1), Kind::Changed),
                (Some(1), Some(2), Kind::Changed),
            ]
        );
        assert_eq!(rows[1].new_words, vec![8..10], "only the number changed");
        assert_eq!(
            rows[2].new_words,
            vec![9..13],
            "the words added, the space between them too"
        );
    }

    #[test]
    fn unchanged_runs_far_from_a_change_fold_and_open() {
        let old: Vec<String> = (0..20).map(|i| format!("line {i}")).collect();
        let mut new = old.clone();
        new[10] = "changed".into();
        let rows = align(&old, &new);
        let shown = items(&rows, &HashSet::new(), false);
        assert_eq!(shown[0], Item::Fold { first: 0, count: 7 });
        assert_eq!(shown[1], Item::Row(7));
        assert_eq!(
            shown.last(),
            Some(&Item::Fold {
                first: 14,
                count: 6
            })
        );
        let opened: HashSet<usize> = [0].into();
        assert_eq!(items(&rows, &opened, false)[0], Item::Row(0));
        assert_eq!(items(&rows, &HashSet::new(), true).len(), 20);
    }

    #[test]
    fn a_long_line_wraps_at_a_space_and_a_word_too_long_where_it_fills() {
        assert_eq!(wrap("the quick brown fox", 10), vec![0..10, 10..19]);
        assert_eq!(wrap("abcdefghijkl", 5), vec![0..5, 5..10, 10..12]);
        assert_eq!(wrap("", 5), vec![0..0]);
        assert_eq!(lines_of("a\tb\r\nc"), vec!["a    b", "c"]);
    }
}
