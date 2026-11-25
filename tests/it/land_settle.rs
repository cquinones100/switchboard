//! queue-and-clean's `switchboard land-lock settle`, in a worktree of a
//! temporary repository whose `origin` is a bare repository standing in for
//! GitHub's (an `insteadOf` from its github.com address), with a temporary
//! HOME and repos.json: a branch already on top lands with nothing rebased
//! and its tested commit not tested again; a clean rebase with `afterRebase`
//! set runs only the check, the default branch passing on a small GitHub of
//! the test's own; a conflict, a failing check and a change that isn't the
//! one asked about each let the lock go, the last putting the worktree
//! first in the queue to review again.

use crate::common;
use crate::land_test::{git, record, repos, said, trust, worktree};

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use common::{BIN, TempDir};
use serde_json::{Value, json};

/// The worktree's repository with `origin` at github.com/o/r, sent to a
/// bare repository in `home`, `main` pushed there and the main clone
/// trusted; the worktree.
fn landing(home: &Path) -> PathBuf {
    let dir = worktree(home);
    let bare = home.join("origin.git");
    git(home, &["init", "-q", "--bare", bare.to_str().unwrap()]);
    let url = "https://github.com/o/r.git";
    git(&dir, &["remote", "add", "origin", url]);
    git(
        &dir,
        &["config", &format!("url.{}.insteadOf", bare.display()), url],
    );
    git(&dir, &["push", "-q", "origin", "main"]);
    git(&dir, &["fetch", "-q", "origin"]);
    trust(home, &home.join("repo"));
    dir
}

/// Another landing's commit on origin's `main`: `file` written with `text`.
fn lands_meanwhile(home: &Path, file: &str, text: &str) -> String {
    let repo = home.join("repo");
    std::fs::write(repo.join(file), text).unwrap();
    git(&repo, &["add", file]);
    git(&repo, &["commit", "-q", "-m", "meanwhile"]);
    git(&repo, &["push", "-q", "origin", "main"]);
    git(&repo, &["rev-parse", "HEAD"])
}

fn switchboard(home: &Path, dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(BIN);
    command
        .args(args)
        .current_dir(dir)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e")
        .env("LAND_LOCK_POLL", "0.05")
        // Nothing reaches the real GitHub: a test that needs it gives its own.
        .env("SWITCHBOARD_GITHUB_API", "http://127.0.0.1:9")
        .env_remove("GITHUB_TOKEN")
        .env_remove("SWITCHBOARD")
        .env_remove("SWITCHBOARD_PANE")
        .env_remove("SWITCHBOARD_SETTINGS")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .stdin(Stdio::null());
    command
}

/// The change from origin's `main` to HEAD, as the landing asked about it.
fn patch(home: &Path, dir: &Path) -> String {
    let out = switchboard(
        home,
        dir,
        &["codex-review", "--patch-id", "origin/main", "HEAD"],
    )
    .output()
    .unwrap();
    assert!(out.status.success(), "{}", said(&out));
    said(&out).trim().to_string()
}

fn settle(home: &Path, dir: &Path, patch: &str) -> Command {
    switchboard(home, dir, &["land-lock", "settle", "--patch", patch])
}

fn holder(home: &Path, dir: &Path) -> String {
    let out = switchboard(home, dir, &["land-lock", "holder"])
        .output()
        .unwrap();
    said(&out).trim().to_string()
}

fn last(out: &Output) -> String {
    said(out).lines().last().unwrap_or_default().to_string()
}

#[test]
fn a_branch_already_on_top_lands_without_a_rebase_or_tests_run_again() {
    let tmp = TempDir::new("land-settle-on-top");
    let home = tmp.path();
    let dir = landing(home);
    let ran = home.join("tests-ran");
    repos(
        home,
        "o/r",
        json!({ "lands": "direct", "review": "none",
            "test": format!("touch {}", ran.display()) }),
    );
    let tested = switchboard(home, &dir, &["land-lock", "test", "--base", "origin/main"])
        .output()
        .unwrap();
    assert_eq!(tested.status.code(), Some(0), "{}", said(&tested));
    std::fs::remove_file(&ran).unwrap();
    let head = git(&dir, &["rev-parse", "HEAD"]);

    let out = settle(home, &dir, &patch(home, &dir)).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
    assert!(last(&out).starts_with("ready "), "{}", said(&out));
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), head, "nothing rebased");
    assert!(!ran.exists(), "the commit tested before isn't tested again");
    assert_eq!(
        holder(home, &dir),
        dir.canonicalize().unwrap().to_string_lossy(),
        "ready to push, it holds the lock"
    );
}

#[test]
fn a_conflict_is_left_to_resolve_with_the_lock_let_go() {
    let tmp = TempDir::new("land-settle-conflict");
    let home = tmp.path();
    let dir = landing(home);
    repos(
        home,
        "o/r",
        json!({ "lands": "direct", "review": "none", "test": "true" }),
    );
    let asked = patch(home, &dir);
    lands_meanwhile(home, "a.txt", "theirs\n");

    let out = settle(home, &dir, &asked).output().unwrap();
    assert_eq!(out.status.code(), Some(10), "{}", said(&out));
    assert!(said(&out).contains("conflict in a.txt"), "{}", said(&out));
    assert_eq!(holder(home, &dir), "", "the lock is let go");
    assert_eq!(record(&dir)["settle"]["conflict"], true);

    // Resolved and continued, the next settle tests it and has it reviewed
    // (here asked) again, however its patch reads.
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    git(&dir, &["add", "a.txt"]);
    let continued = Command::new("git")
        .args(["-c", "core.editor=true", "rebase", "--continue"])
        .current_dir(&dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e")
        .output()
        .unwrap();
    assert!(continued.status.success());
    let out = settle(home, &dir, &patch(home, &dir)).output().unwrap();
    assert_eq!(out.status.code(), Some(12), "{}", said(&out));
    assert_eq!(
        record(&dir)["tests"]["head"],
        git(&dir, &["rev-parse", "HEAD"])
    );
    assert_eq!(record(&dir)["settle"]["conflict"], false);
}

#[test]
fn a_change_other_than_the_one_asked_about_is_requeued() {
    let tmp = TempDir::new("land-settle-changed");
    let home = tmp.path();
    let dir = landing(home);
    repos(
        home,
        "o/r",
        json!({ "lands": "direct", "review": "none", "test": "true" }),
    );

    let out = settle(home, &dir, "0000").output().unwrap();
    assert_eq!(out.status.code(), Some(12), "{}", said(&out));
    assert_eq!(last(&out), "changed");
    assert_eq!(holder(home, &dir), "", "the lock is let go");
    let common = home.join("repo/.git");
    let queue: Value =
        serde_json::from_str(&std::fs::read_to_string(common.join("land.queue")).unwrap()).unwrap();
    assert_eq!(
        queue[0]["reviewing"], true,
        "first in line, reviewing: {queue}"
    );
}

#[cfg(feature = "net")]
mod after_rebase {
    use super::*;
    use common::http::{self, Reply};
    use common::stand_in;

    /// A GitHub whose every commit has `runs` for its check runs.
    fn github(runs: Value) -> http::Fake {
        http::Fake::github("test-token", move |request| {
            if request.method != "GET" || !request.target.starts_with("/repos/o/r/commits/") {
                return None;
            }
            let body = if request.target.contains("/check-runs") {
                json!({ "total_count": runs.as_array().map_or(0, Vec::len), "check_runs": runs })
            } else {
                json!({ "state": "pending", "statuses": [] })
            };
            Some(Reply::json(200, body))
        })
    }

    fn with_github(mut command: Command, home: &Path, fake: &http::Fake) -> Command {
        command
            .env("PATH", stand_in::path_with_gh(&home.join("bin")))
            .env("GH_TOKEN", "test-token")
            .env("SWITCHBOARD_GITHUB_API", &fake.url);
        command
    }

    fn passing() -> Value {
        json!([{ "name": "Tests", "status": "completed", "conclusion": "success",
            "details_url": "https://github.com/o/r/actions/runs/77/job/5" }])
    }

    #[test]
    fn a_clean_rebase_runs_only_the_check() {
        let tmp = TempDir::new("land-settle-check");
        let home = tmp.path();
        let dir = landing(home);
        let (tests, checked) = (home.join("tests-ran"), home.join("check-ran"));
        repos(
            home,
            "o/r",
            json!({ "lands": "direct", "review": "none",
                "test": format!("touch {}", tests.display()),
                "afterRebase": format!("touch {}", checked.display()) }),
        );
        let asked = patch(home, &dir);
        let main = lands_meanwhile(home, "b.txt", "b\n");
        let fake = github(passing());

        let out = with_github(settle(home, &dir, &asked), home, &fake)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", said(&out));
        assert_eq!(git(&dir, &["rev-parse", "HEAD~1"]), main, "rebased");
        assert!(checked.exists(), "the check ran");
        assert!(!tests.exists(), "the tests didn't");
    }

    #[test]
    fn a_failing_check_lets_the_lock_go() {
        let tmp = TempDir::new("land-settle-check-fails");
        let home = tmp.path();
        let dir = landing(home);
        repos(
            home,
            "o/r",
            json!({ "lands": "direct", "review": "none", "test": "true",
                "afterRebase": "echo broken; false" }),
        );
        let asked = patch(home, &dir);
        lands_meanwhile(home, "b.txt", "b\n");
        let fake = github(passing());

        let out = with_github(settle(home, &dir, &asked), home, &fake)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(11), "{}", said(&out));
        assert!(said(&out).contains("broken"), "{}", said(&out));
        assert!(last(&out).starts_with("check failed "), "{}", said(&out));
        assert!(last(&out).ends_with("landing-check.log"));
        assert_eq!(holder(home, &dir), "", "the lock is let go");
    }
}
