//! Keeping an open note and its file in step when something outside the app
//! changes the file, such as a script or Claude marking a checklist item,
//! ported from notes/lua/notes/sync.lua.
//!
//! The open note remembers its base: the file as it was last read or
//! written. A file that no longer matches its base was changed from
//! outside. A note with nothing unsaved takes the change in place; one with
//! unsaved typing gets both merged, line by line (`merge`). Every save
//! checks first (the app's `check`), so nothing is written over a change the
//! app hasn't seen, and a watch on the notes directory (`watch`) checks as
//! soon as a file changes.

use std::path::Path;

use similar::{Algorithm, DiffTag, capture_diff_slices};

fn lines_of(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// What changed from `from` to `to`: lines `lo..hi` of `from` (`lo == hi`
/// putting lines in before `lo`) become `lines`, as Neovim's `vim.diff`
/// gave its hunks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub lo: usize,
    pub hi: usize,
    pub lines: Vec<String>,
    /// 0 for ours, 1 for theirs.
    side: u8,
}

pub fn hunks(from: &[&str], to: &[&str]) -> Vec<Hunk> {
    hunks_of(from, to, 0)
}

fn hunks_of(from: &[&str], to: &[&str], side: u8) -> Vec<Hunk> {
    let mut out: Vec<Hunk> = Vec::new();
    let mut open: Option<(usize, usize, usize, usize)> = None;
    for op in capture_diff_slices(Algorithm::Myers, from, to) {
        let (tag, old, new) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            if let Some((lo, hi, nlo, nhi)) = open.take() {
                out.push(Hunk {
                    lo,
                    hi,
                    lines: to[nlo..nhi].iter().map(|s| s.to_string()).collect(),
                    side,
                });
            }
            continue;
        }
        open = Some(match open {
            Some((lo, _, nlo, _)) => (lo, old.end, nlo, new.end),
            None => (old.start, old.end, new.start, new.end),
        });
    }
    if let Some((lo, hi, nlo, nhi)) = open {
        out.push(Hunk {
            lo,
            hi,
            lines: to[nlo..nhi].iter().map(|s| s.to_string()).collect(),
            side,
        });
    }
    out
}

/// Merges two edits of `original`: changes to different lines both apply,
/// even on lines next to each other, as when you type in one checklist item
/// while something outside marks the next. Where both changed the same
/// lines differently, both versions are kept, yours first, rather than
/// either being lost. Gives the text and how many such places there were.
pub fn merge(original: &str, ours: &str, theirs: &str) -> (String, usize) {
    let from = lines_of(original);
    let mut all = hunks_of(&from, &lines_of(ours), 0);
    all.extend(hunks_of(&from, &lines_of(theirs), 1));
    all.sort_by_key(|h| (h.lo, h.hi, h.side));
    // Hunks that touch the same lines, or put lines in at the same place.
    struct Group {
        lo: usize,
        hi: usize,
        hunks: Vec<Hunk>,
    }
    let mut groups: Vec<Group> = Vec::new();
    for h in all {
        match groups.last_mut() {
            Some(g) if h.lo < g.hi || (h.lo == h.hi && g.lo == g.hi && h.lo == g.lo) => {
                g.hi = g.hi.max(h.hi);
                g.hunks.push(h);
            }
            _ => groups.push(Group {
                lo: h.lo,
                hi: h.hi,
                hunks: vec![h],
            }),
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut at = 0;
    let mut clashes = 0;
    // A group's lines as one side has them.
    let side_of = |g: &Group, side: u8| {
        let mut lines: Vec<String> = Vec::new();
        let mut pos = g.lo;
        for h in g.hunks.iter().filter(|h| h.side == side) {
            lines.extend(from[pos..h.lo.max(pos)].iter().map(|s| s.to_string()));
            lines.extend(h.lines.iter().cloned());
            pos = h.hi.max(pos);
        }
        lines.extend(from[pos..g.hi.max(pos)].iter().map(|s| s.to_string()));
        lines
    };
    for g in &groups {
        out.extend(from[at..g.lo.max(at)].iter().map(|s| s.to_string()));
        let (mine, other) = (side_of(g, 0), side_of(g, 1));
        let ours = g.hunks.iter().any(|h| h.side == 0);
        let theirs = g.hunks.iter().any(|h| h.side == 1);
        if ours && theirs && mine != other {
            clashes += 1;
            out.extend(mine);
            out.extend(other);
        } else {
            out.extend(if ours { mine } else { other });
        }
        at = g.hi.max(at);
    }
    out.extend(from[at..].iter().map(|s| s.to_string()));
    let text = if out.is_empty() {
        String::new()
    } else {
        format!("{}\n", out.join("\n"))
    };
    (text, clashes)
}

/// The rows of `after` an edit from `before` wrote, for sorting a ticked
/// item brought in from outside under the rest.
pub fn changed_rows(before: &[String], after: &[String]) -> Vec<usize> {
    let mut rows = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, before, after) {
        let (tag, _, new) = op.as_tag_tuple();
        if tag != DiffTag::Equal {
            rows.extend(new);
        }
    }
    rows
}

/// Watches `root` and everything under it, calling `changed` on every
/// change; the app waits a moment after the last before it looks, as the
/// plugin did. The watch stops when what this gives is dropped. `None`
/// where the system can't watch, and then the app looks when it gets the
/// focus back and before each save.
pub fn watch(
    root: &Path,
    changed: impl Fn() + Send + 'static,
) -> Option<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok() {
            changed();
        }
    })
    .ok()?;
    watcher.watch(root, RecursiveMode::Recursive).ok()?;
    Some(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_edits_merge_line_by_line_and_both_versions_of_one_line_are_kept() {
        let base = "# List\n- [ ] one\n- [ ] two\n- [ ] three\n";
        assert_eq!(
            merge(
                base,
                "# List\n- [ ] one, typed\n- [ ] two\n- [ ] three\n",
                "# List\n- [ ] one\n- [ ] two (ABC-1)\n- [ ] three\n"
            ),
            (
                "# List\n- [ ] one, typed\n- [ ] two (ABC-1)\n- [ ] three\n".into(),
                0
            ),
            "next to each other both apply"
        );
        assert_eq!(
            merge(
                base,
                "# List\n- [ ] one\n- [ ] two, mine\n- [ ] three\n",
                "# List\n- [ ] one\n- [ ] two (ABC-1)\n- [ ] three\n"
            ),
            (
                "# List\n- [ ] one\n- [ ] two, mine\n- [ ] two (ABC-1)\n- [ ] three\n".into(),
                1
            ),
            "on the same line both versions are kept, yours first"
        );
        assert_eq!(
            merge(
                base,
                "# List\n- [x] one\n- [ ] two\n- [ ] three\n",
                "# List\n- [x] one\n- [ ] two\n- [ ] three\n"
            ),
            ("# List\n- [x] one\n- [ ] two\n- [ ] three\n".into(), 0),
            "the same change made on both sides appears once"
        );
        assert_eq!(
            merge(
                base,
                &format!("{base}- [ ] mine\n"),
                &format!("{base}- [ ] theirs\n")
            ),
            (
                "# List\n- [ ] one\n- [ ] two\n- [ ] three\n- [ ] mine\n- [ ] theirs\n".into(),
                1
            )
        );
        assert_eq!(
            merge(
                base,
                "# List\n- [ ] two\n- [ ] three\n",
                "# List\n- [ ] one\n- [ ] two\n- [ ] three\n- [ ] four\n"
            ),
            ("# List\n- [ ] two\n- [ ] three\n- [ ] four\n".into(), 0)
        );
    }
}
