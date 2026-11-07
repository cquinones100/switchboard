//! M-p's "Clear this tab's diff cache": what M-d's diff keeps of a tab's
//! branch, removed, so its next opening asks Haiku again (ported from
//! tmux/diff-cache-clear.sh and diffview.lua's `clear`). That is Haiku's
//! order of its files (`order::order_file`), its story's answer
//! (`diff-story/<key>.txt`, by `story::key_of`) and the PR base kept for
//! the worktree (`git::forget_base`), each found from the same change M-d
//! would read, by the same functions M-d's diff and the story told ahead use,
//! so the clear always finds what they kept. Nothing is told again until
//! M-d opens: the story told ahead keeps its own note of the commit it told.
//!
//! A story being told right now is kept, since its telling would keep it
//! again, and what is said says so. The base is looked for as M-d looks for
//! it (kept, else fetched), never asked of GitHub with the github setting
//! off; when the change can't be read (no base, a diff git won't give),
//! what is said says why rather than reporting the story and order cleared.
//! The tmux version kept a parsed story and an index beside each answer,
//! which the port's diff doesn't, so there is nothing more to clear.
//!
//! Blocking: it runs git, so the server runs it off its task.

use std::path::Path;

use super::{git, order, story, tell};

/// What the clear did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cleared {
    /// How many files went.
    pub removed: usize,
    /// The story was being told, so it was kept.
    pub telling: bool,
    /// Why the story and order couldn't be reached.
    pub missed: Option<String>,
    /// Whether the PR base kept for the worktree went.
    pub based: bool,
}

/// Clears what M-d keeps of the branch checked out in `dir`; `Err` with
/// what to say when `dir` is in no repository.
pub fn clear(dir: &Path) -> Result<Cleared, String> {
    let facts = git::facts(dir)
        .map_err(|_| "Not in a git repository, so there is no diff cache here.".to_string())?;
    let mut done = Cleared::default();
    let remove = |file: &Path, done: &mut Cleared| {
        if std::fs::remove_file(file).is_ok() {
            done.removed += 1;
        }
    };
    // The base as M-d finds it: kept for this commit, else fetched.
    let base = git::cached_base(dir).or_else(|| git::fetch_base(dir).map(|b| b.base));
    match base.map(|b| git::read_change(&facts.top, &b)) {
        None => done.missed = Some("its PR base couldn't be worked out".into()),
        Some(Err(e)) => done.missed = Some(format!("its diff couldn't be read ({e})")),
        Some(Ok(change)) => {
            let files = order::ordered(&change);
            if !files.is_empty() {
                let file = order::order_file(&change.top, &change.left, &change.right, &files);
                remove(&file, &mut done);
            }
            let (key, _) = story::key_of(&change);
            if tell::being_told(&key) {
                done.telling = true;
            } else {
                remove(&tell::story_dir().join(format!("{key}.txt")), &mut done);
            }
        }
    }
    if git::forget_base(&facts.git_dir) {
        done.based = true;
        done.removed += 1;
    }
    Ok(done)
}

/// What the box says of a clear, as diff-cache-clear.sh said it.
pub fn words(done: &Cleared) -> String {
    if let Some(missed) = &done.missed {
        let base = if done.based {
            " Its PR base is cleared."
        } else {
            ""
        };
        return format!("Couldn't clear this tab's diff story and file order: {missed}.{base}");
    }
    let count = match done.removed {
        0 => "Nothing kept for this tab's diff".to_string(),
        1 => "This tab's diff cache is cleared: 1 file".to_string(),
        n => format!("This tab's diff cache is cleared: {n} files"),
    };
    if done.telling {
        format!(
            "{count}. Its story is being told right now, so it was kept: clear it again once M-d shows it."
        )
    } else {
        format!("{count}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_said_counts_the_files_and_says_why_a_story_was_kept() {
        let said = |removed, telling, missed: Option<&str>, based| {
            words(&Cleared {
                removed,
                telling,
                missed: missed.map(str::to_string),
                based,
            })
        };
        assert_eq!(
            said(0, false, None, false),
            "Nothing kept for this tab's diff."
        );
        assert_eq!(
            said(3, false, None, true),
            "This tab's diff cache is cleared: 3 files."
        );
        assert!(
            said(1, true, None, true)
                .starts_with("This tab's diff cache is cleared: 1 file. Its story is being told right now, so it was kept")
        );
        assert_eq!(
            said(1, false, Some("its PR base couldn't be worked out"), true),
            "Couldn't clear this tab's diff story and file order: its PR base couldn't be worked out. Its PR base is cleared."
        );
        assert_eq!(
            clear(Path::new("/")),
            Err("Not in a git repository, so there is no diff cache here.".into())
        );
    }
}
