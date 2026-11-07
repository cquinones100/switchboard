//! Whether the PR has every commit made here, for the second status line:
//! "↑2 to push", or "pushed". tmux/tab-status.sh counted the commits ahead
//! of the branch's upstream (`git rev-list --count @{upstream}..HEAD`, else
//! of origin's copy of the branch) on every redraw, against what was last
//! fetched or pushed, so it never touched the network.
//!
//! Here the two commits are read from git's own files on the server's tick:
//! HEAD's, and the upstream's, from the branch's `remote` and `merge` in the
//! repository's config, else `origin/<branch>`. The same commit on both is
//! "pushed" at once; any other pair is counted by git once, off the server's
//! task, and kept by the pair, since the commits between two commits never
//! change. So drawing the line reads only what was kept, and git runs only
//! after a commit is made, pushed or fetched.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::tabs::git;

/// Kept counts past this many are let go, all but the ones in use.
const KEEP: usize = 64;

/// A branch's commit and its upstream's.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Pair {
    pub head: String,
    pub upstream: String,
}

/// The commit a ref names: its own file (in the checkout's git directory
/// or the shared one), else its line of `packed-refs`.
fn commit_of(git_dir: &Path, common_dir: &Path, name: &str) -> Option<String> {
    for dir in [git_dir, common_dir] {
        if let Ok(text) = fs::read_to_string(dir.join(name)) {
            let text = text.trim();
            if let Some(other) = text.strip_prefix("ref:") {
                return commit_of(git_dir, common_dir, other.trim());
            }
            return Some(text.to_string()).filter(|c| !c.is_empty());
        }
    }
    let packed = fs::read_to_string(common_dir.join("packed-refs")).ok()?;
    packed
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with('^'))
        .find_map(|l| {
            let (commit, r) = l.split_once(' ')?;
            (r.trim() == name).then(|| commit.to_string())
        })
}

/// Where a branch's upstream is kept: its `remote` and `merge` in the
/// repository's config (a remote of `.` being this repository itself),
/// else origin's copy of a branch of the same name.
fn upstream_ref(common_dir: &Path, branch: &str) -> String {
    let fallback = format!("refs/remotes/origin/{branch}");
    let Ok(config) = fs::read_to_string(common_dir.join("config")) else {
        return fallback;
    };
    let section = format!("[branch \"{branch}\"]");
    let (mut inside, mut remote, mut merge) = (false, None, None);
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == section;
            continue;
        }
        if !inside {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim().to_lowercase().as_str() {
            "remote" => remote = Some(value.trim().to_string()),
            "merge" => merge = Some(value.trim().to_string()),
            _ => {}
        }
    }
    match (remote, merge) {
        (Some(r), Some(m)) if r == "." => m,
        (Some(r), Some(m)) => {
            let name = m.strip_prefix("refs/heads/").unwrap_or(&m);
            format!("refs/remotes/{r}/{name}")
        }
        _ => fallback,
    }
}

/// A directory's branch commit and its upstream's, from git's files; `None`
/// off a branch or with no upstream.
pub fn pair(dir: &Path) -> Option<Pair> {
    let place = git::locate(dir)?;
    if !place.on_branch() {
        return None;
    }
    let head = commit_of(
        &place.git_dir,
        &place.common_dir,
        &format!("refs/heads/{}", place.branch),
    )?;
    let upstream = commit_of(
        &place.git_dir,
        &place.common_dir,
        &upstream_ref(&place.common_dir, &place.branch),
    )?;
    Some(Pair { head, upstream })
}

/// The commits on `pair.head` that `pair.upstream` lacks, counted by git;
/// blocking, so it runs off the server's task.
pub fn count(dir: &Path, pair: &Pair) -> Option<u32> {
    let out = crate::util::output_of(
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["--no-optional-locks", "rev-list", "--count"])
            .arg(format!("{}..{}", pair.upstream, pair.head)),
    )
    .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// What the server keeps of the counts.
#[derive(Debug, Default)]
pub struct Pushes {
    /// Each directory's two commits, as last read.
    pairs: HashMap<PathBuf, Pair>,
    /// The commits ahead, by pair.
    counts: HashMap<Pair, u32>,
    /// The pairs git is counting now, and those it couldn't, which aren't
    /// asked about again.
    asking: HashSet<Pair>,
    failed: HashSet<Pair>,
}

impl Pushes {
    /// Reads a directory's commits again; gives the pair git has to count,
    /// when nothing is known of it yet.
    pub fn look(&mut self, dir: &Path) -> Option<Pair> {
        let Some(p) = pair(dir) else {
            self.pairs.remove(dir);
            return None;
        };
        self.pairs.insert(dir.to_path_buf(), p.clone());
        if self.counts.len() > KEEP {
            let used: HashSet<&Pair> = self.pairs.values().collect();
            self.counts.retain(|k, _| used.contains(k));
        }
        let known = p.head == p.upstream
            || self.counts.contains_key(&p)
            || self.failed.contains(&p)
            || !self.asking.insert(p.clone());
        (!known).then_some(p)
    }

    /// Git's count for a pair, or `None` when it couldn't count.
    pub fn counted(&mut self, pair: Pair, count: Option<u32>) {
        self.asking.remove(&pair);
        match count {
            Some(n) => {
                self.counts.insert(pair, n);
            }
            None => {
                self.failed.insert(pair);
            }
        }
    }

    /// The commits a directory has to push: 0 when every one is pushed,
    /// `None` when that isn't known.
    pub fn ahead(&self, dir: &Path) -> Option<u32> {
        let p = self.pairs.get(dir)?;
        if p.head == p.upstream {
            return Some(0);
        }
        self.counts.get(p).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabs::git::tests::{Scratch, git, repo};

    fn head(dir: &Path) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn the_commits_ahead_of_origin_are_counted_once_and_pushed_needs_no_count() {
        let scratch = Scratch::new("push");
        let dir = scratch.0.join("app");
        if !repo(&dir) {
            eprintln!("skipped: no git");
            return;
        }
        let mut pushes = Pushes::default();
        assert_eq!(pushes.look(&dir), None, "no upstream: nothing to say");
        assert_eq!(pushes.ahead(&dir), None);

        // origin's copy at the same commit: pushed, with nothing to count.
        assert!(git(
            &dir,
            &["update-ref", "refs/remotes/origin/main", "HEAD"]
        ));
        assert_eq!(pushes.look(&dir), None);
        assert_eq!(pushes.ahead(&dir), Some(0));

        // Two commits on: counted by git, once.
        assert!(git(&dir, &["commit", "-q", "--allow-empty", "-m", "a"]));
        assert!(git(&dir, &["commit", "-q", "--allow-empty", "-m", "b"]));
        let p = pushes.look(&dir).expect("a pair to count");
        assert_eq!(p.head, head(&dir));
        assert_eq!(pushes.ahead(&dir), None, "not known until counted");
        assert_eq!(pushes.look(&dir), None, "asked once");
        pushes.counted(p.clone(), count(&dir, &p));
        assert_eq!(pushes.ahead(&dir), Some(2));
        assert_eq!(pushes.look(&dir), None);

        // Packed refs read as loose ones do.
        assert!(git(&dir, &["pack-refs", "--all"]));
        assert_eq!(pair(&dir), Some(p));

        // A failed count isn't asked for again.
        let odd = Pair {
            head: "x".into(),
            upstream: "y".into(),
        };
        pushes.counted(odd.clone(), None);
        assert!(pushes.failed.contains(&odd));
    }

    #[test]
    fn the_branchs_own_upstream_wins_over_origins_copy() {
        let scratch = Scratch::new("upstream");
        let dir = scratch.0.join("app");
        if !repo(&dir) {
            eprintln!("skipped: no git");
            return;
        }
        let first = head(&dir);
        assert!(git(&dir, &["commit", "-q", "--allow-empty", "-m", "a"]));
        assert!(git(
            &dir,
            &["update-ref", "refs/remotes/origin/main", "HEAD"]
        ));
        assert!(git(&dir, &["update-ref", "refs/remotes/up/trunk", &first]));
        assert!(git(&dir, &["config", "branch.main.remote", "up"]));
        assert!(git(
            &dir,
            &["config", "branch.main.merge", "refs/heads/trunk"]
        ));
        let p = pair(&dir).unwrap();
        assert_eq!(p.upstream, first);
        assert_eq!(count(&dir, &p), Some(1));
        // A local branch as upstream.
        assert!(git(&dir, &["branch", "-q", "base", &first]));
        assert!(git(&dir, &["config", "branch.main.remote", "."]));
        assert!(git(
            &dir,
            &["config", "branch.main.merge", "refs/heads/base"]
        ));
        assert_eq!(pair(&dir).unwrap().upstream, first);
        // Off a branch: nothing.
        assert!(git(&dir, &["checkout", "-q", "--detach"]));
        assert_eq!(pair(&dir), None);
    }
}
