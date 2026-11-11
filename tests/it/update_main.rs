//! `switchboard update-main`, ported from test/update-main.test.js: a
//! throwaway origin (a bare repository) and a clone of it with a worktree, in
//! a temporary directory, so nothing touches your own repositories.

use crate::common;

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use common::{BIN, TempDir};

fn env(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e");
}

fn git(home: &Path, dir: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args).stderr(Stdio::null());
    env(&mut cmd, home);
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// (code, stdout, stderr)
fn run(home: &Path, dir: &Path) -> (i32, String, String) {
    let mut cmd = Command::new(BIN);
    cmd.arg("update-main").current_dir(dir).stdin(Stdio::null());
    env(&mut cmd, home);
    let out = cmd.output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn main_is_fast_forwarded_where_it_is_checked_out_and_a_dirty_checkout_is_left_alone() {
    let tmp = TempDir::new("update-main");
    let home = tmp.path();
    let s = |p: &Path| p.to_str().unwrap().to_string();
    let origin = home.join("origin.git");
    let clone = home.join("clone");
    let other = home.join("other");
    let worktree = home.join("worktree");
    git(home, home, &["init", "-q", "--bare", &s(&origin)]);
    // On main, as `init -b main` (which git before 2.28 lacks) would have it.
    git(home, &origin, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(home, home, &["clone", "-q", &s(&origin), &s(&clone)]);
    git(home, &clone, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(
        home,
        &clone,
        &["commit", "-q", "--allow-empty", "-m", "start"],
    );
    git(home, &clone, &["push", "-q", "origin", "main"]);
    git(home, &clone, &["remote", "set-head", "origin", "-a"]);
    git(
        home,
        &clone,
        &["worktree", "add", "-q", "-b", "feat", &s(&worktree)],
    );
    // Someone else moves origin's main on.
    git(home, home, &["clone", "-q", &s(&origin), &s(&other)]);
    git(
        home,
        &other,
        &["commit", "-q", "--allow-empty", "-m", "theirs"],
    );
    git(home, &other, &["push", "-q", "origin", "main"]);
    let theirs = git(home, &other, &["rev-parse", "HEAD"]);

    let (code, out, err) = run(home, &worktree);
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("main: "), "{out}");
    assert_eq!(
        git(home, &clone, &["rev-parse", "main"]),
        theirs,
        "main caught up in the main clone"
    );
    assert_eq!(
        git(home, &worktree, &["branch", "--show-current"]),
        "feat",
        "and the worktree stayed on its branch"
    );
    let (_, again, _) = run(home, &worktree);
    assert!(again.contains("up to date"), "{again}");

    git(
        home,
        &other,
        &["commit", "-q", "--allow-empty", "-m", "more"],
    );
    git(home, &other, &["push", "-q", "origin", "main"]);
    fs::write(clone.join("x"), "work in progress\n").unwrap();
    git(home, &clone, &["add", "x"]);
    let (code, _, err) = run(home, &worktree);
    assert_eq!(code, 1);
    assert!(
        err.contains("changes not committed, so it was left"),
        "{err}"
    );
    assert_eq!(
        git(home, &clone, &["rev-parse", "main"]),
        theirs,
        "left where it was"
    );
}

#[test]
fn more_than_one_directory_is_a_usage_error() {
    let tmp = TempDir::new("update-main-usage");
    let mut cmd = Command::new(BIN);
    cmd.args(["update-main", "a", "b"]).current_dir(tmp.path());
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Usage: update-main [<dir>]"));
    let outside = Command::new(BIN)
        .arg("update-main")
        .current_dir(tmp.path())
        .env("GIT_CEILING_DIRECTORIES", "/tmp")
        .output()
        .unwrap();
    assert_eq!(outside.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&outside.stderr).contains("update-main: not in a git repository")
    );
}
