//! A commit's story told ahead of time, so the diff opens on the story at
//! once rather than waiting on Haiku: the diff's part, ported from
//! tmux/diff-story-ahead.sh. The workspace hands the diff a tab's new
//! commit (the plugin API's `Event::Commit`, `switchboard diff --ahead`),
//! one branch at a time (`switchboard`'s src/diff/ahead.rs decides which).
//!
//! It compares with the same base the diff would (git.rs), tells the story
//! as the diff would (tell.rs), keeping it in the cache by the diff's own
//! text, and is done once it is told. A story told with a warning (its
//! answer mostly named hunks the diff doesn't have) is told, the warning
//! given back. The commit told is kept per worktree, and a second run on
//! the same one, or while one runs, does nothing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::tell::{Asked, Heard, Teller};
use super::{git, order, story};
use crate::{paths, util};

/// How long the story is given, its own wait and a bit.
const GIVE_UP: Duration = Duration::from_secs(330);

// ── The program ─────────────────────────────────────────────────────────────

fn state_file(git_dir: &Path) -> PathBuf {
    paths::state_dir()
        .join("diff-story-ahead")
        .join(git_dir.to_string_lossy().replace('/', "%"))
}

/// The lock that keeps a worktree to one run at a time, taken away however
/// the run ends.
struct Lock(PathBuf);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

/// Tells the story of the commit the branch in `dir` is at, once, with the
/// tab's plan note `plan` ("" for none) as its context: any warning it was
/// told with, or why it couldn't be told.
pub fn commit(dir: &Path, plan: &str) -> Result<Option<String>, String> {
    let Ok(facts) = git::facts(dir) else {
        return Ok(None);
    };
    let state = state_file(&facts.git_dir);
    // The commit told and the prompt's version it was told with, so a new
    // prompt tells it again.
    let marker = format!("{} v{}", facts.head, story::VERSION);
    if util::read_text(&state).is_some_and(|t| t.trim() == marker) {
        return Ok(None);
    }
    let lock = PathBuf::from(format!("{}.lock", state.display()));
    let _ = std::fs::create_dir_all(state.parent().unwrap_or(Path::new("/")));
    if std::fs::create_dir(&lock).is_err() {
        return Ok(None);
    }
    let _lock = Lock(lock);
    // A base that couldn't be fetched isn't the one M-d will compare with
    // once it can: left to try again rather than told against the old one.
    let base = match git::cached_base(&facts.top) {
        Some(b) => b,
        None => match git::fetch_base(&facts.top) {
            Some(found) if found.fetched => found.base,
            Some(found) => return Err(format!("Could not fetch {}", found.base)),
            None => return Err("Couldn't work out the PR's base branch".into()),
        },
    };
    let warning = tell_ahead(&facts.top, &base, plan)?;
    let _ = util::write_atomic(&state, format!("{marker}\n"));
    Ok(warning)
}

/// Tells the story of the change against `base`, as M-d would, the tab's
/// plan note `plan` ("" for none) its context; gives any warning it was
/// told with, or why it couldn't be told.
pub fn tell_ahead(top: &Path, base: &str, plan: &str) -> Result<Option<String>, String> {
    let change = git::read_change(top, base).map_err(|e| e.to_string())?;
    let (key, paths) = story::key_of(&change);
    // Nothing to tell: told, as far as the next commit is concerned.
    if paths.len() < 2 || paths.iter().all(|p| order::generated(p)) {
        return Ok(None);
    }
    let (diffs, hunks) = story::split(&[change.diff.as_str()]);
    let entries: Vec<story::Entry> = change
        .entries
        .iter()
        .map(|e| story::Entry {
            path: e.path.clone(),
            additions: e.additions,
            deletions: e.deletions,
        })
        .collect();
    let commits = story::commits(&change.raw_commits);
    let asked = Asked {
        key,
        input: story::input(
            &entries,
            &diffs,
            &hunks,
            &commits,
            &story::plan_context(Some(plan)),
        ),
        paths,
        hunks,
    };
    let teller = Teller::default();
    let (tx, rx) = std::sync::mpsc::channel::<Heard>();
    {
        let teller = teller.clone();
        std::thread::spawn(move || {
            teller.tell(&asked, &|heard| {
                if !matches!(heard, Heard::Progress(_)) {
                    let _ = tx.send(heard);
                }
            });
        });
    }
    let heard = rx.recv_timeout(GIVE_UP);
    teller.stop();
    match heard {
        Ok(Heard::Told(story)) => Ok(story::missed_text(&story)),
        Ok(Heard::Untold(why)) => Err(why),
        Ok(Heard::Progress(_)) => Err("the story stopped".into()),
        Err(_) => Err(format!(
            "gave up waiting after {} seconds",
            GIVE_UP.as_secs()
        )),
    }
}
