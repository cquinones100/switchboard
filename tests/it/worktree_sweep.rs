//! Landed worktrees removed on their own (src/worktree_sweep): a tab closed
//! in a worktree whose work is on the default branch takes the worktree
//! with it, its branch kept; with Settings' `sweep_landed_worktrees` off,
//! it stays.

use crate::common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use common::TestServer;

/// Runs git in `cwd`, with none of your own config.
fn git(cwd: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A clone of a bare origin with main pushed, and a worktree under its
/// `.claude/worktrees` on a branch with nothing main lacks, git last used
/// in it two hours ago.
fn landed_worktree(server: &TestServer) -> (PathBuf, PathBuf) {
    let top = server.dir.path();
    let origin = top.join("origin.git");
    let repo = top.join("repo");
    git(
        top,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    git(
        top,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            repo.to_str().unwrap(),
        ],
    );
    git(&repo, &["checkout", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    git(&repo, &["push", "-q", "origin", "main"]);
    let tree = repo.join(".claude/worktrees/done");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "done",
            tree.to_str().unwrap(),
        ],
    );
    let own = repo.join(".git/worktrees/done");
    let then = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    for f in ["index", "HEAD", "logs/HEAD"] {
        std::fs::File::options()
            .write(true)
            .open(own.join(f))
            .unwrap()
            .set_modified(then)
            .unwrap();
    }
    (repo, tree)
}

/// Opens a tab in `tree`, waits for the server to know it is there, and
/// closes it.
fn open_and_close(server: &TestServer, tree: &Path) {
    let opened = server.ok(&["new-tab", "-d", "-c", tree.to_str().unwrap()]);
    let tab = opened.split_whitespace().next().unwrap().to_string();
    let want = tree.canonicalize().unwrap();
    let deadline = common::deadline(Duration::from_secs(10));
    loop {
        let list = server.list();
        let there = list["sessions"][0]["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| {
                t["path"]
                    .as_str()
                    .and_then(|p| Path::new(p).canonicalize().ok())
                    == Some(want.clone())
            });
        if there {
            break;
        }
        assert!(Instant::now() < deadline, "the tab in the worktree: {list}");
        std::thread::sleep(Duration::from_millis(50));
    }
    server.ok(&["kill-tab", "-t", &tab]);
}

#[test]
fn a_tab_closed_in_a_landed_worktree_takes_the_worktree_and_keeps_its_branch() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (repo, tree) = landed_worktree(&server);
    open_and_close(&server, &tree);
    let deadline = common::deadline(Duration::from_secs(10));
    while tree.exists() {
        assert!(Instant::now() < deadline, "the landed worktree removed");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(git(&repo, &["branch", "--list", "done"]).contains("done"));
    client.wait_text("Removed 1 landed worktree");
}

#[test]
fn with_the_setting_off_a_landed_worktree_stays() {
    let server = TestServer::start_with_settings(r#"{ "sweep_landed_worktrees": false }"#);
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (_repo, tree) = landed_worktree(&server);
    open_and_close(&server, &tree);
    // Nothing to wait on: as long as the sweep takes when it is on.
    std::thread::sleep(Duration::from_secs(2) * common::patience());
    assert!(tree.exists());
}

/// A stand-in for GitHub saying every PR asked about by number is merged.
fn github_says_merged() -> common::http::Fake {
    common::http::Fake::github("test-token", |request| {
        if request.path() != "/graphql" {
            return None;
        }
        let answer = if request.text().contains("pullRequest(number:") {
            serde_json::json!({"data": {"repository": {"pullRequest": {
                "number": 12345, "url": "https://github.com/o/r/pull/12345", "title": "t",
                "state": "MERGED", "isDraft": false, "baseRefName": "main", "headRefName": "x"
            }}}})
        } else {
            serde_json::json!({"data": {}})
        };
        Some(common::http::Reply::json(200, answer))
    })
}

/// A clone of `o/r` on GitHub, and PR 12345 checked out in the review
/// folder of the server's home, as `wtpr --untrusted` lays it out.
fn review_checkout(server: &TestServer) -> (PathBuf, PathBuf) {
    let repo = server.dir.path().join("r");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    git(
        &repo,
        &["remote", "add", "origin", "git@github.com:o/r.git"],
    );
    let tree = server.home.join(".cache/switchboard/pr-review/o-r/12345");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "pr-review/12345",
            tree.to_str().unwrap(),
        ],
    );
    (repo, tree)
}

fn start_reviewing(settings: &str) -> (TestServer, common::http::Fake) {
    let fake = github_says_merged();
    let server = TestServer::start_with_env_and_settings(
        &[
            ("GH_TOKEN", "test-token"),
            ("SWITCHBOARD_GITHUB_GRAPHQL", &fake.graphql_url()),
            ("SWITCHBOARD_GITHUB_API", &fake.url),
        ],
        settings,
    );
    (server, fake)
}

#[test]
fn a_tab_closed_in_a_merged_prs_review_checkout_takes_it_and_its_branch() {
    let (server, _fake) = start_reviewing(r#"{ "pause_when_away": false }"#);
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (repo, tree) = review_checkout(&server);
    open_and_close(&server, &tree);
    let deadline = common::deadline(Duration::from_secs(20));
    while tree.exists() {
        assert!(Instant::now() < deadline, "the review checkout removed");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!git(&repo, &["branch", "--list", "pr-review/*"]).contains("12345"));
    client.wait_text("Removed the review checkout of 1 finished PR");
}

#[test]
fn with_the_setting_off_a_merged_prs_review_checkout_stays() {
    let (server, _fake) =
        start_reviewing(r#"{ "pause_when_away": false, "sweep_pr_reviews": false }"#);
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (repo, tree) = review_checkout(&server);
    open_and_close(&server, &tree);
    // Nothing to wait on: as long as the sweep takes when it is on.
    std::thread::sleep(Duration::from_secs(3) * common::patience());
    assert!(tree.exists());
    assert!(git(&repo, &["branch", "--list", "pr-review/*"]).contains("12345"));
}
