//! Someone else's PR checked out for review (`wtpr --untrusted`, M-p's
//! "Review a contributor's PR") removed once it is finished with, which
//! nothing did before: each checkout under `~/.cache/switchboard/pr-review/
//! <owner>-<repo>/<number>` stayed until removed by hand, as the review-pr
//! skill's last step offered, and they piled up, gigabytes of them.
//!
//! A checkout goes when its PR is merged or closed, or when no tab has been
//! in it for `UNSEEN`, and in either case only when no pane has it open now,
//! no `switchboard try` runs on it, nothing in it is uncommitted or
//! untracked, and its main clone lists it as a worktree on its own
//! `pr-review/<number>` branch, unlocked. When a tab was last in one is the
//! server's record (`pr-reviews.json` in the state folder), stamped each
//! time the sweep finds a pane in it, else when it was checked out (its
//! `.git` file's time), since git barely touches a checkout someone only
//! reads. Removal is the review-pr skill's: its try ended and its home
//! removed, `git worktree remove --force` from the main clone (the checks
//! above being what `--force` would otherwise guard), and its branch
//! deleted. Nothing runs from the checkout but `git status`, under its own
//! worktree config, which turns off its hooks, filters and file-system
//! monitor.
//!
//! The rule is here, on plain data and git; `serve` puts it on the server.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::util;

/// The setting that turns this sweep on.
pub const SETTING: &str = "sweep_pr_reviews";

/// The record of when a tab was last in each checkout, in the state folder.
pub const RECORD: &str = "pr-reviews.json";

/// How long a checkout no tab has been in is kept, whatever its PR.
pub const UNSEEN: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// One review checkout: where it is, the main clone it is a worktree of,
/// its repository's folder (`<owner>-<repo>`) and its PR's number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkout {
    pub tree: PathBuf,
    pub main: PathBuf,
    pub folder: String,
    pub number: u64,
}

impl Checkout {
    /// Its branch, as `wtpr --untrusted` names it.
    pub fn branch(&self) -> String {
        format!("pr-review/{}", self.number)
    }
}

/// The checkouts in the review folder `dir`: `<folder>/<number>`, each a
/// worktree whose main clone is found.
pub fn checkouts(dir: &Path) -> Vec<Checkout> {
    let mut found = Vec::new();
    for repo in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let folder = repo.file_name().to_string_lossy().into_owned();
        for pr in std::fs::read_dir(repo.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let Ok(number) = pr.file_name().to_string_lossy().parse::<u64>() else {
                continue;
            };
            let tree = pr.path();
            if let Some(main) = super::main_clone_of(&tree) {
                found.push(Checkout {
                    tree,
                    main,
                    folder: folder.clone(),
                    number,
                });
            }
        }
    }
    found.sort_by(|a, b| a.tree.cmp(&b.tree));
    found
}

/// When a tab was last in each checkout, by its path, in milliseconds.
pub type Seen = BTreeMap<String, u64>;

fn ms(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The record in `file`, each checkout a pane has open stamped `now`, and
/// the checkouts gone left out; written back when it changed.
pub fn stamp(file: &Path, checkouts: &[Checkout], open: &[PathBuf], now: SystemTime) -> Seen {
    let before: Seen = std::fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let mut seen = Seen::new();
    for c in checkouts {
        let key = c.tree.to_string_lossy().into_owned();
        let at = if super::is_open(&c.tree, open) {
            Some(ms(now))
        } else {
            before.get(&key).copied()
        };
        if let Some(at) = at {
            seen.insert(key, at);
        }
    }
    if seen != before
        && let Ok(text) = serde_json::to_string_pretty(&seen)
    {
        let _ = util::write_atomic(file, text + "\n");
    }
    seen
}

/// When a tab was last in `c`: the record's, else when it was checked out.
fn last_seen(c: &Checkout, seen: &Seen) -> Option<u64> {
    seen.get(c.tree.to_string_lossy().as_ref())
        .copied()
        .or_else(|| {
            std::fs::symlink_metadata(c.tree.join(".git"))
                .and_then(|m| m.modified())
                .ok()
                .map(ms)
        })
}

/// Whether a PR in GitHub's `state` is finished with.
pub fn finished(state: Option<&str>) -> bool {
    matches!(state, Some("MERGED" | "CLOSED"))
}

/// Whether no tab has been in `c` for `UNSEEN` at `now`.
fn unseen(c: &Checkout, seen: &Seen, now: SystemTime) -> bool {
    last_seen(c, seen).is_some_and(|at| ms(now).saturating_sub(at) >= UNSEEN.as_millis() as u64)
}

/// git in `dir`, reading only, never taking the index's lock.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = util::output_of(
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "core.fsmonitor=false"])
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0"),
    )
    .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The try's home for the checkout at `tree`, as `switchboard try` names it.
fn try_home(tree: &Path) -> PathBuf {
    let real = tree.canonicalize().unwrap_or_else(|_| tree.to_path_buf());
    crate::cmd::try_::home_of(Path::new(crate::cmd::try_::ROOT), &real)
}

/// Whether a `switchboard try` of `tree` runs: something listens on its
/// home's socket. Connecting sends nothing.
fn try_runs(tree: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(crate::cmd::try_::socket_of(&try_home(tree))).is_ok()
}

/// Whether `c` must stay whatever its PR: a pane has it open, a try runs
/// on it, its main clone doesn't list it on its own branch (or lists it
/// locked), or it holds something uncommitted or untracked.
pub fn kept(c: &Checkout, open: &[PathBuf]) -> bool {
    if super::is_open(&c.tree, open) || try_runs(&c.tree) {
        return true;
    }
    let real = c.tree.canonicalize().unwrap_or_else(|_| c.tree.clone());
    let listed = git(&c.main, &["worktree", "list", "--porcelain"])
        .map(|p| super::parse(&p))
        .unwrap_or_default();
    let its_own = listed.iter().any(|w| {
        w.path.canonicalize().ok().as_ref() == Some(&real)
            && w.branch.as_deref() == Some(c.branch().as_str())
            && !w.locked
    });
    !its_own || git(&c.tree, &["status", "--porcelain"]).is_none_or(|s| !s.trim().is_empty())
}

/// The checkouts to remove at `now`, their PRs' states asked of `state`
/// (none when it can't say), each checked by `kept` first, so GitHub is
/// asked only about the ones that could go.
pub fn due(
    checkouts: &[Checkout],
    open: &[PathBuf],
    seen: &Seen,
    now: SystemTime,
    state: &mut dyn FnMut(&Checkout) -> Option<String>,
) -> Vec<Checkout> {
    checkouts
        .iter()
        .filter(|c| !kept(c, open))
        .filter(|c| unseen(c, seen, now) || finished(state(c).as_deref()))
        .cloned()
        .collect()
}

/// The bytes the files under `dir` take, links not followed.
pub fn size(dir: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(dir) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| size(&e.path()))
        .sum::<u64>()
        + meta.len()
}

/// `c` removed as the review-pr skill removes it: its try ended and its
/// home removed, the worktree removed from its main clone and its branch
/// deleted. Gives the bytes freed, or none when git refused.
pub fn remove(c: &Checkout) -> Option<u64> {
    let home = try_home(&c.tree);
    if home.is_dir() || crate::cmd::try_sealed::is_sealed(&home) {
        let _ = crate::cmd::try_::clean(&home);
    }
    let bytes = size(&c.tree);
    let removed = util::output_of(
        Command::new("git")
            .arg("-C")
            .arg(&c.main)
            .args(["worktree", "remove", "--force"])
            .arg(&c.tree),
    )
    .is_ok_and(|o| o.status.success());
    if !removed {
        return None;
    }
    let _ = util::output_of(Command::new("git").arg("-C").arg(&c.main).args([
        "branch",
        "-D",
        &c.branch(),
    ]));
    // The repository's folder goes with its last checkout.
    if let Some(folder) = c.tree.parent() {
        let _ = std::fs::remove_dir(folder);
    }
    Some(bytes)
}

/// What the server says once the sweep removed something.
pub fn words(removed: &[(Checkout, u64)]) -> String {
    let n = removed.len();
    let bytes: u64 = removed.iter().map(|(_, b)| b).sum();
    let numbers: Vec<String> = removed
        .iter()
        .map(|(c, _)| format!("#{}", c.number))
        .collect();
    format!(
        "Removed the review checkout{} of {n} finished PR{}, {}: {}",
        if n == 1 { "" } else { "s" },
        if n == 1 { "" } else { "s" },
        crate::usage::resources::bytes(bytes),
        numbers.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@e")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@e")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// A main clone with one commit, and a review folder beside it.
    struct Setup {
        dir: crate::mux::socket::testing::TempDir,
        main: PathBuf,
    }

    impl Setup {
        fn new() -> Setup {
            let dir = crate::mux::socket::testing::TempDir::new();
            let main = dir.path().join("repo");
            std::fs::create_dir(&main).unwrap();
            run(&main, &["init", "-q", "-b", "main"]);
            std::fs::write(main.join("a.txt"), "a\n").unwrap();
            run(&main, &["add", "a.txt"]);
            run(&main, &["commit", "-q", "-m", "start"]);
            let main = main.canonicalize().unwrap();
            Setup { dir, main }
        }

        fn reviews(&self) -> PathBuf {
            self.dir.path().join("pr-review")
        }

        /// PR `number` checked out as `wtpr --untrusted` lays it out.
        fn checkout(&self, number: u64) -> Checkout {
            let tree = self.reviews().join("o-r").join(number.to_string());
            run(
                &self.main,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    &format!("pr-review/{number}"),
                    tree.to_str().unwrap(),
                ],
            );
            Checkout {
                tree,
                main: self.main.clone(),
                folder: "o-r".into(),
                number,
            }
        }

        fn branches(&self) -> String {
            run(&self.main, &["branch", "--list", "pr-review/*"])
        }
    }

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    #[test]
    fn a_finished_prs_checkout_with_no_tab_goes_with_its_branch_and_the_rest_stay() {
        let s = Setup::new();
        let merged = s.checkout(1);
        let with_a_tab = s.checkout(2);
        let uncommitted = s.checkout(3);
        let open_seen_yesterday = s.checkout(4);
        let untracked = s.checkout(5);
        std::fs::write(uncommitted.tree.join("a.txt"), "changed\n").unwrap();
        std::fs::write(untracked.tree.join("new.txt"), "new\n").unwrap();
        let found = checkouts(&s.reviews());
        assert_eq!(found.len(), 5);
        assert_eq!(found[0], merged);

        let now = SystemTime::now();
        let file = s.dir.path().join("pr-reviews.json");
        // A tab was in 4 yesterday.
        stamp(
            &file,
            &found,
            std::slice::from_ref(&open_seen_yesterday.tree),
            now - DAY,
        );
        std::fs::create_dir(with_a_tab.tree.join("src")).unwrap();
        let open = vec![with_a_tab.tree.join("src")];
        let seen = stamp(&file, &found, &open, now);
        let mut asked = Vec::new();
        let mut state = |c: &Checkout| {
            asked.push(c.number);
            Some(if c.number == 4 { "OPEN" } else { "MERGED" }.to_string())
        };
        let going = due(&found, &open, &seen, now, &mut state);
        assert_eq!(going, vec![merged.clone()]);
        // GitHub is asked only about the ones nothing else keeps.
        assert_eq!(asked, vec![1, 4]);

        let freed = remove(&merged).unwrap();
        assert!(freed > 0);
        assert!(!merged.tree.exists());
        assert!(!s.branches().contains("pr-review/1"));
        assert!(s.branches().contains("pr-review/2"));
        assert!(with_a_tab.tree.exists() && uncommitted.tree.exists());
    }

    #[test]
    fn a_checkout_no_tab_has_been_in_for_two_weeks_goes_whatever_its_pr() {
        let s = Setup::new();
        let old = s.checkout(7);
        let found = checkouts(&s.reviews());
        let file = s.dir.path().join("pr-reviews.json");
        let now = SystemTime::now();
        let mut open_pr = |_: &Checkout| Some("OPEN".to_string());
        let mut unknown = |_: &Checkout| None;

        // Checked out just now, it stays; its checkout's time counts as seen.
        let seen = stamp(&file, &found, &[], now);
        assert!(due(&found, &[], &seen, now, &mut open_pr).is_empty());
        let later = now + UNSEEN + DAY;
        assert_eq!(
            due(&found, &[], &seen, later, &mut unknown),
            vec![old.clone()]
        );

        // A tab in it a day before then keeps it.
        let seen = stamp(&file, &found, std::slice::from_ref(&old.tree), later - DAY);
        assert!(due(&found, &[], &seen, later, &mut open_pr).is_empty());
        assert!(due(&found, &[], &seen, later + UNSEEN, &mut open_pr) == vec![old]);
    }

    #[test]
    fn the_record_forgets_checkouts_gone() {
        let s = Setup::new();
        let one = s.checkout(1);
        let file = s.dir.path().join("pr-reviews.json");
        let now = SystemTime::now();
        let seen = stamp(
            &file,
            std::slice::from_ref(&one),
            std::slice::from_ref(&one.tree),
            now,
        );
        assert_eq!(seen.len(), 1);
        assert!(stamp(&file, &[], &[], now).is_empty());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{}\n");
    }

    #[test]
    fn only_merged_or_closed_is_finished() {
        assert!(finished(Some("MERGED")) && finished(Some("CLOSED")));
        assert!(!finished(Some("OPEN")) && !finished(None));
    }

    #[test]
    fn the_words_count_the_checkouts_and_their_size() {
        let c = |n| Checkout {
            tree: PathBuf::new(),
            main: PathBuf::new(),
            folder: String::new(),
            number: n,
        };
        let gb = 1024 * 1024 * 1024;
        assert_eq!(
            words(&[(c(3), gb)]),
            "Removed the review checkout of 1 finished PR, 1.0 GB: #3"
        );
        assert_eq!(
            words(&[(c(3), gb), (c(4), gb / 2)]),
            "Removed the review checkouts of 2 finished PRs, 1.5 GB: #3, #4"
        );
    }
}
