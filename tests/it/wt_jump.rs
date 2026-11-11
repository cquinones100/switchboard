//! `wt-jump` against a test server: tabs open in a ticket's and a PR's
//! worktrees, listed with where each is, and a jump by the ticket or the
//! PR's number making that tab the current one.

use crate::common;

use std::fs;
use std::path::Path;
use std::process::Command;

use common::{BIN, TestServer};
use serde_json::Value;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `switchboard wt-jump <args>` as run in a pane of the server.
fn jump(server: &TestServer, pane: &str, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(BIN)
        .arg("wt-jump")
        .args(args)
        .env("HOME", &server.home)
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env("SWITCHBOARD", server.socket())
        .env("SWITCHBOARD_PANE", pane)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn current_tab(server: &TestServer) -> String {
    let list = server.list();
    list["sessions"][0]["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["current"] == Value::Bool(true))
        .and_then(|t| t["id"].as_str())
        .unwrap()
        .to_string()
}

#[test]
fn tabs_are_found_by_ticket_and_pr_and_listed() {
    let server = TestServer::start();
    let repo = server.dir.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    let ticket = repo.join(".claude/worktrees/eng-12-fix-login");
    let pr = repo.join(".claude/worktrees/pr-7-feat");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "eng-12-fix-login",
            &ticket.to_string_lossy(),
        ],
    );
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "feat", &pr.to_string_lossy()],
    );

    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let made = |dir: &Path| {
        let out = server.ok(&["new-tab", "-d", "-c", dir.to_str().unwrap()]);
        let mut ids = out.split_whitespace().map(str::to_string);
        (ids.next().unwrap(), ids.next().unwrap())
    };
    let (ticket_tab, ticket_pane) = made(&ticket);
    let (pr_tab, _) = made(&pr);
    let home_tab = current_tab(&server);
    let home_pane = server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (code, out, err) = jump(&server, &home_pane, &["--list"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.lines().count(), 3, "a row a tab: {out}");
    assert!(
        out.lines().all(|l| l.starts_with("main:")),
        "each where it is: {out}"
    );
    assert!(out.lines().any(|l| l.ends_with("(here)")), "{out}");

    let (code, _, err) = jump(&server, &home_pane, &["ENG-12"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(current_tab(&server), ticket_tab, "the ticket's tab");

    let (code, _, err) = jump(&server, &ticket_pane, &["7"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        current_tab(&server),
        pr_tab,
        "the PR's, by its pr-<n> worktree"
    );

    let (code, _, err) = jump(&server, &home_pane, &["ENG-99"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("no open tab for ENG-99") && err.contains("wt ENG-99"),
        "{err}"
    );
    assert_ne!(current_tab(&server), home_tab);

    // Outside Switchboard it only lists.
    let out = Command::new(BIN)
        .args(["wt-jump", "ENG-12"])
        .env("HOME", &server.home)
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env_remove("SWITCHBOARD")
        .env_remove("SWITCHBOARD_PANE")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not inside Switchboard"));
}
