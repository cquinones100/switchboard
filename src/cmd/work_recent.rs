//! What the work pickers last had picked, so their lists lead with it: the
//! directories any of them was given (M-t, M-n, M-W, M-B, wtpr's, wtn's,
//! shaping's), and, for each repository, the branches picked at "Land on:"
//! (wtn's picker and autopilot's setup), and, for each note, the repository
//! its items were last started in. New in the Rust version: the tmux
//! version's pick-dir.zsh and autopilot's target list had no memory, so the
//! repository items are queued in sat wherever its tabs put it, and a
//! branch like `team/next` had to be found each time.
//!
//! One small file, `work-recent.json` in the state folder, the most recent
//! first in each list, with the time each was picked: `{"dirs": [{"path",
//! "at"}], "branches": {"<common git dir>": [{"name", "at"}]}, "notes":
//! {"<note's ID>": "<repository>"}}`, a note with no ID known by its path
//! within the notes folder. The default branch is kept as `""`, the value
//! its row carries. Each list keeps its
//! last 50, and a directory that is gone is dropped as the next pick is kept.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde_json::{Value, json};

use crate::{paths, util};

/// How many picks each list keeps.
const MAX: usize = 50;

pub fn file() -> PathBuf {
    paths::state_dir().join("work-recent.json")
}

/// The directories picked, the most recent first.
pub fn dirs() -> Vec<PathBuf> {
    dirs_in(&file())
}

/// The branches picked at "Land on:" for the repository whose common git
/// directory is `common`, the most recent first; `""` is the default one.
pub fn branches(common: &Path) -> Vec<String> {
    branches_in(&file(), common)
}

/// Keeps `dir` as the directory picked last.
pub fn pick_dir(dir: &Path) {
    pick_dir_in(&file(), dir, util::now_ms());
}

/// Keeps `branch` (`""` for the default one) as the branch picked last in
/// the repository whose common git directory is `common`.
pub fn pick_branch(common: &Path, branch: &str) {
    pick_branch_in(&file(), common, branch, util::now_ms());
}

/// The repository a note's items were last started in, `note` its ID or,
/// with none, its path within the notes folder; `None` once it is gone.
pub fn note_repo(note: &str) -> Option<PathBuf> {
    note_repo_in(&file(), note)
}

/// Keeps `repo` as the repository a note's items were last started in.
pub fn pick_note_repo(note: &str, repo: &Path) {
    pick_note_repo_in(&file(), note, repo);
}

/// Runs `keep` (one of the picks above) off the server's task, or right
/// away where there is no runtime.
pub fn in_background(keep: impl FnOnce() + Send + 'static) {
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::spawn_blocking(keep);
    } else {
        keep();
    }
}

/// Held while a pick is read in and written back, so two kept at once (a
/// branch and the note's repository, as an item's tab opens) don't write
/// over each other.
fn writing() -> MutexGuard<'static, ()> {
    static WRITING: Mutex<()> = Mutex::new(());
    WRITING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn read(file: &Path) -> Value {
    Value::Object(util::read_json_object(file))
}

/// A list's entries, the most recent first, each with its `key` field.
fn entries<'a>(list: &'a Value, key: &str) -> Vec<(&'a str, i64)> {
    let mut out: Vec<(&str, i64)> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| Some((e[key].as_str()?, e["at"].as_i64().unwrap_or(0))))
        .collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.1));
    out
}

fn dirs_in(file: &Path) -> Vec<PathBuf> {
    entries(&read(file)["dirs"], "path")
        .into_iter()
        .map(|(p, _)| PathBuf::from(p))
        .collect()
}

fn branches_in(file: &Path, common: &Path) -> Vec<String> {
    let all = read(file);
    entries(&all["branches"][common.to_string_lossy().as_ref()], "name")
        .into_iter()
        .map(|(b, _)| b.to_string())
        .collect()
}

/// `list` with `value` first at `at`, every other entry `keep` says yes to
/// after it, the most recent first, at most `MAX`.
fn put_first(list: &Value, key: &str, value: &str, at: i64, keep: impl Fn(&str) -> bool) -> Value {
    let mut out = vec![json!({ key: value, "at": util::js_number(at as f64) })];
    out.extend(
        entries(list, key)
            .into_iter()
            .filter(|(v, _)| *v != value && keep(v))
            .map(|(v, t)| json!({ key: v, "at": util::js_number(t as f64) })),
    );
    out.truncate(MAX);
    Value::Array(out)
}

fn pick_dir_in(file: &Path, dir: &Path, at: i64) {
    let _held = writing();
    let mut all = read(file);
    all["dirs"] = put_first(&all["dirs"], "path", &dir.to_string_lossy(), at, |p| {
        Path::new(p).is_dir()
    });
    let _ = util::write_atomic(file, util::json_pretty(&all));
}

fn pick_branch_in(file: &Path, common: &Path, branch: &str, at: i64) {
    let _held = writing();
    let mut all = read(file);
    if !all["branches"].is_object() {
        all["branches"] = json!({});
    }
    let repo = common.to_string_lossy().to_string();
    all["branches"][&repo] = put_first(&all["branches"][&repo], "name", branch, at, |_| true);
    let _ = util::write_atomic(file, util::json_pretty(&all));
}

fn note_repo_in(file: &Path, note: &str) -> Option<PathBuf> {
    read(file)["notes"][note]
        .as_str()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

fn pick_note_repo_in(file: &Path, note: &str, repo: &Path) {
    let _held = writing();
    let mut all = read(file);
    if !all["notes"].is_object() {
        all["notes"] = json!({});
    }
    all["notes"][note] = json!(repo.to_string_lossy());
    let _ = util::write_atomic(file, util::json_pretty(&all));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sb-work-recent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn directories_keep_the_last_fifty_newest_first_dropping_those_gone() {
        let dir = scratch("dirs");
        let file = dir.join("work-recent.json");
        let gone = dir.join("gone");
        std::fs::create_dir_all(&gone).unwrap();
        pick_dir_in(&file, &gone, 1);
        std::fs::remove_dir_all(&gone).unwrap();
        for i in 0..60 {
            let d = dir.join(format!("d{i}"));
            std::fs::create_dir_all(&d).unwrap();
            pick_dir_in(&file, &d, 10 + i);
        }
        let kept = dirs_in(&file);
        assert_eq!(kept.len(), MAX);
        assert_eq!(kept[0], dir.join("d59"));
        assert!(!kept.contains(&gone));
        // Picking one again moves it to the front, once.
        pick_dir_in(&file, &dir.join("d30"), 100);
        let kept = dirs_in(&file);
        assert_eq!(kept[0], dir.join("d30"));
        assert_eq!(kept.iter().filter(|d| **d == dir.join("d30")).count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn branches_are_kept_for_each_repository_the_default_as_empty() {
        let dir = scratch("branches");
        let file = dir.join("work-recent.json");
        let (a, b) = (Path::new("/r/a/.git"), Path::new("/r/b/.git"));
        pick_branch_in(&file, a, "", 1);
        pick_branch_in(&file, a, "team/next", 2);
        pick_branch_in(&file, b, "x", 3);
        assert_eq!(branches_in(&file, a), ["team/next", ""]);
        assert_eq!(branches_in(&file, b), ["x"]);
        assert!(branches_in(&file, Path::new("/r/c/.git")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_note_keeps_the_repository_its_items_were_last_started_in() {
        let dir = scratch("notes");
        let file = dir.join("work-recent.json");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        assert_eq!(note_repo_in(&file, "id-1"), None);
        pick_note_repo_in(&file, "id-1", &a);
        pick_note_repo_in(&file, "Plan.md", &a);
        pick_note_repo_in(&file, "id-1", &b);
        assert_eq!(note_repo_in(&file, "id-1"), Some(b.clone()));
        assert_eq!(note_repo_in(&file, "Plan.md"), Some(a));
        // Kept beside the other lists, and dropped once the folder is gone.
        pick_dir_in(&file, &b, 1);
        assert_eq!(dirs_in(&file), std::slice::from_ref(&b));
        std::fs::remove_dir_all(&b).unwrap();
        assert_eq!(note_repo_in(&file, "id-1"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
