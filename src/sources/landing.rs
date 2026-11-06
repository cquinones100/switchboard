//! The landing queue of queue-and-clean's "Land on the default branch",
//! ported from lib/sidebar/landing.js: which tab's worktree holds the
//! landing lock and which wait for it, in order, read from the files
//! `land-lock` keeps in the main clone's git directory (`land.lock/holder`
//! and `land.queue`, claude/skills/queue-and-clean/scripts/land-lock.js).
//! Found from files alone, with no git process: a worktree's `.git` file
//! names its git directory, whose `commondir` leads to the shared one.
//!
//! A holder or waiter counts while its worktree is there and a Claude
//! session runs in it, as land-lock.js's `gone()` has it, so a landing that
//! ended without a word leaves the group before the script next reads the
//! queue. The sessions come from Claude Code's session files and a signal 0
//! to each one's process. A session in a worktree nested inside another
//! (the main clone's `.claude/worktrees`) is that one's, not the outer one's.
//!
//! The server runs this off its task on every tick, since it reads a few
//! files per repository and every session file.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::facts::Landing;
use crate::mux::TabId;
use crate::util;

/// A waiting tab's worktree and whether it is first or last in line, for
/// its menu's moves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waiter {
    pub root: PathBuf,
    pub first: bool,
    pub last: bool,
}

/// A tab's place in its repository's landing queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub landing: Landing,
    pub waiter: Option<Waiter>,
}

/// A directory's worktree (`root`) and its repository's common git
/// directory, with links resolved, or `None` outside a git repository.
pub fn git_of(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    for at in dir.ancestors() {
        let dot_git = at.join(".git");
        let Ok(meta) = fs::metadata(&dot_git) else {
            continue;
        };
        let root = at.canonicalize().ok()?;
        if meta.is_dir() {
            return Some((root, dot_git.canonicalize().ok()?));
        }
        let text = fs::read_to_string(&dot_git).ok()?;
        let named = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
        let gitdir = at.join(named);
        // A submodule or a --separate-git-dir clone has no commondir: its
        // git directory is the shared one.
        let common = match fs::read_to_string(gitdir.join("commondir")) {
            Ok(text) => gitdir.join(text.trim()),
            Err(_) => gitdir,
        };
        return Some((root, common.canonicalize().ok()?));
    }
    None
}

/// The working directories of the running Claude sessions, from the session
/// files in `dir` (`~/.claude/sessions`) whose process is still there.
pub fn running_sessions(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| util::read_json(e.path()))
        .filter(|s| {
            s.get("pid")
                .and_then(Value::as_i64)
                .is_some_and(util::is_alive)
        })
        .filter_map(|s| s.get("cwd").and_then(Value::as_str).map(PathBuf::from))
        .collect()
}

// Whether a worktree is there and a session runs in it (not in a worktree
// nested inside it).
fn live_in(root: &Path, running: &[PathBuf]) -> bool {
    root.exists()
        && running
            .iter()
            .any(|cwd| cwd.starts_with(root) && git_of(cwd).is_some_and(|(at, _)| at == root))
}

/// Each tab's place in its repository's landing queue, from the first of
/// its directories in a git repository, given the running sessions'
/// directories. Each repository's files are read once.
pub fn landing_of(tabs: &[(TabId, Vec<PathBuf>)], running: &[PathBuf]) -> HashMap<TabId, Place> {
    let mut repos: HashMap<PathBuf, (Option<PathBuf>, Vec<PathBuf>)> = HashMap::new();
    let mut places = HashMap::new();
    for (tab, dirs) in tabs {
        let Some((root, common)) = dirs.iter().find_map(|d| git_of(d)) else {
            continue;
        };
        let (holder, queue) = repos.entry(common.clone()).or_insert_with(|| {
            let live = |p: &Path| live_in(p, running);
            let holder = util::read_json(common.join("land.lock/holder"))
                .and_then(|h| h.get("path").and_then(Value::as_str).map(PathBuf::from))
                .filter(|p| live(p));
            let queue = util::read_json(common.join("land.queue"))
                .and_then(|q| q.as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|e| e.get("path").and_then(Value::as_str).map(PathBuf::from))
                .filter(|p| live(p))
                .collect();
            (holder, queue)
        });
        if holder.as_deref() == Some(root.as_path()) {
            places.insert(
                *tab,
                Place {
                    landing: Landing {
                        holding: true,
                        line: 1,
                    },
                    waiter: None,
                },
            );
        } else if let Some(at) = queue.iter().position(|p| *p == root) {
            let line = at as u32 + if holder.is_some() { 2 } else { 1 };
            places.insert(
                *tab,
                Place {
                    landing: Landing {
                        holding: false,
                        line,
                    },
                    waiter: Some(Waiter {
                        root,
                        first: at == 0,
                        last: at + 1 == queue.len(),
                    }),
                },
            );
        }
    }
    places
}

/// "landing", or "3rd in line to land", for a tab's words.
pub fn landing_words(landing: &Landing) -> String {
    if landing.holding {
        return "landing".into();
    }
    let n = landing.line;
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix} in line to land")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabs::git::tests::{Scratch, git, repo};

    #[test]
    fn the_landing_one_first_then_the_queue_in_order_and_a_gone_waiter_leaves() {
        // lib/sidebar's "tabs landing on the default branch or waiting their
        // turn are a Landing group in the landing queue's order".
        let scratch = Scratch::new("landing");
        let main = scratch.0.join("repo");
        if !repo(&main) {
            eprintln!("skipped: no git");
            return;
        }
        let sessions = scratch.0.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let pid = std::process::id();
        let [one, two, three] = ["one", "two", "three"].map(|name| {
            let worktree = scratch.0.join(name);
            assert!(git(
                &main,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    name,
                    worktree.to_str().unwrap()
                ]
            ));
            fs::write(
                sessions.join(format!("landing-{name}.json")),
                serde_json::json!({ "sessionId": name, "cwd": worktree, "pid": pid }).to_string(),
            )
            .unwrap();
            worktree
        });
        let common = main.join(".git");
        fs::create_dir_all(common.join("land.lock")).unwrap();
        fs::write(
            common.join("land.lock/holder"),
            serde_json::json!({ "path": one, "since": 1 }).to_string(),
        )
        .unwrap();
        fs::write(
            common.join("land.queue"),
            serde_json::json!([{ "path": three, "since": 2 }, { "path": two, "since": 3 }])
                .to_string(),
        )
        .unwrap();
        fs::create_dir_all(two.join("sub")).unwrap();
        let tabs = vec![
            (TabId(1), vec![scratch.0.clone()]),
            (TabId(2), vec![two.join("sub")]),
            (TabId(3), vec![one.clone()]),
            (TabId(4), vec![three.clone()]),
        ];
        let running = running_sessions(&sessions);
        let places = landing_of(&tabs, &running);
        let line = |id: u32| places.get(&TabId(id)).map(|p| p.landing.line);
        assert_eq!(places.get(&TabId(1)), None, "a tab outside the repository");
        assert!(places[&TabId(3)].landing.holding);
        assert_eq!((line(3), line(4), line(2)), (Some(1), Some(2), Some(3)));
        assert_eq!(
            landing_words(&places[&TabId(2)].landing),
            "3rd in line to land",
            "a tab in a worktree's subdirectory included"
        );
        assert_eq!(landing_words(&places[&TabId(3)].landing), "landing");
        let first = places[&TabId(4)].waiter.as_ref().unwrap();
        assert!(
            first.first && !first.last,
            "the first in line moves only down"
        );
        assert_eq!(first.root, three);

        // Notes beside the landing tab and focused: the tab is found by the
        // first of its directories in a repository.
        let beside = landing_of(
            &[(TabId(3), vec![scratch.0.join("nowhere"), one.clone()])],
            &running,
        );
        assert!(beside[&TabId(3)].landing.holding);

        // A waiter whose Claude has gone leaves before the script reads the
        // queue again.
        fs::remove_file(sessions.join("landing-three.json")).unwrap();
        let places = landing_of(&tabs, &running_sessions(&sessions));
        let mut lines: Vec<(u32, u32)> =
            places.iter().map(|(t, p)| (t.0, p.landing.line)).collect();
        lines.sort();
        assert_eq!(lines, vec![(2, 2), (3, 1)]);
    }

    #[test]
    fn ordinals() {
        let words = |line| {
            landing_words(&Landing {
                holding: false,
                line,
            })
        };
        assert_eq!(words(2), "2nd in line to land");
        assert_eq!(words(11), "11th in line to land");
        assert_eq!(words(21), "21st in line to land");
    }
}
