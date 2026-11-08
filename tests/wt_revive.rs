//! `wt-revive`: the worktrees no tab has open, from a repository and the
//! Claude transcripts of a temporary HOME, with no server (no tab is open)
//! and with a test server whose tab sits in one of them (left out); and its
//! cache, read back on the next look.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{BIN, TempDir, TestServer};
use serde_json::{Value, json};

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

/// A repository with two worktrees, and a transcript of a session in the
/// first, under `home`.
fn layout(root: &Path, home: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    // Beside the main clone, so the session in the first is that
    // worktree's alone (one nested in the main clone counts for both).
    let one = root.join("one");
    let two = root.join("two");
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "one", &one.to_string_lossy()],
    );
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "two", &two.to_string_lossy()],
    );
    // Last touched a day ago, so the session's worktree is the newest.
    let day_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(86_400);
    for dir in [&repo, &two] {
        fs::File::open(dir).unwrap().set_modified(day_ago).unwrap();
    }
    let project = home.join(".claude/projects/-x");
    fs::create_dir_all(&project).unwrap();
    let now = jiff::Timestamp::now().to_string();
    let lines = [
        json!({ "type": "user", "cwd": one.join("src"), "timestamp": now, "message": { "content": "make the thing do what it should, please" } }),
        json!({ "type": "user", "cwd": one.join("src"), "timestamp": now, "message": { "content": "yes" } }),
    ];
    fs::write(
        project.join("a.jsonl"),
        lines.map(|l| l.to_string()).join("\n"),
    )
    .unwrap();
    (repo, one, two)
}

fn run(dir: &Path, home: &Path, state: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(BIN)
        .arg("wt-revive")
        .args(args)
        .current_dir(dir)
        .env("HOME", home)
        .env("SWITCHBOARD_STATE_DIR", state)
        .env_remove("SWITCHBOARD")
        .env_remove("SWITCHBOARD_PANE")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn rows(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn every_worktree_with_no_tab_newest_first_with_its_last_prompt() {
    let tmp = TempDir::new("wt-revive");
    let home = tmp.path().join("home");
    let (repo, one, two) = layout(tmp.path(), &home);
    let state = tmp.path().join("s");

    let (code, out, err) = run(&repo, &home, &state, &["--json"]);
    assert_eq!(code, 0, "{err}");
    let found = rows(&out);
    let mut trees: Vec<&str> = found.iter().map(|r| r["tree"].as_str().unwrap()).collect();
    assert_eq!(trees[0], one.to_str().unwrap(), "the one used last first");
    trees.sort();
    let mut every = vec![
        one.to_str().unwrap(),
        repo.to_str().unwrap(),
        two.to_str().unwrap(),
    ];
    every.sort();
    assert_eq!(trees, every, "every worktree, the main clone's included");
    let first = &found[0];
    let keys: Vec<&String> = first.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        ["when", "tree", "repo", "branch", "label", "age", "prompt"],
        "the fields in the order the Work map reads them"
    );
    assert_eq!(first["branch"], "one");
    assert_eq!(first["label"], "repo one");
    assert_eq!(first["repo"], repo.to_str().unwrap());
    assert_eq!(
        first["prompt"], "make the thing do what it should, please",
        "the last prompt that says something, not a yes"
    );
    assert!(
        found
            .iter()
            .any(|r| r["tree"] == two.to_str().unwrap()
                && r["prompt"] == "(no recent Claude session)")
    );

    let cache: Value = serde_json::from_str(
        &fs::read_to_string(home.join(".cache/wt-revive/index.json")).unwrap(),
    )
    .unwrap();
    let entry = cache["transcripts"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert_eq!(entry["facts"], 2);
    assert_eq!(entry["cwd"], one.join("src").to_str().unwrap());
    assert_eq!(entry["prompt"], "make the thing do what it should, please");

    // Plain rows: label, age and prompt, tab-separated.
    let (code, plain, _) = run(&repo, &home, &state, &[]);
    assert_eq!(code, 0);
    assert!(plain.lines().next().unwrap().starts_with("repo one\t"));

    // Outside any repository, with none in the transcripts either.
    let empty_home = tmp.path().join("empty-home");
    fs::create_dir_all(&empty_home).unwrap();
    let elsewhere = tmp.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let (code, out, _) = run(&elsewhere, &empty_home, &state, &["--json"]);
    assert_eq!((code, out.as_str()), (0, ""), "--json says nothing");
    let (code, _, err) = run(&elsewhere, &empty_home, &state, &[]);
    assert_eq!(code, 1);
    assert!(err.contains("No git repositories found"));
}

#[test]
fn a_worktree_a_tab_is_open_in_is_left_out() {
    let server = TestServer::start();
    let home = server.home.clone();
    let (repo, one, two) = layout(server.dir.path(), &home);
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    server.ok(&["new-tab", "-d", "-c", one.to_str().unwrap()]);
    let (code, out, err) = run(&repo, &home, &server.state, &["--json"]);
    assert_eq!(code, 0, "{err}");
    let trees: Vec<String> = rows(&out)
        .iter()
        .map(|r| r["tree"].as_str().unwrap().to_string())
        .collect();
    assert!(
        !trees.contains(&one.to_string_lossy().into_owned()),
        "{trees:?}"
    );
    assert!(trees.contains(&two.to_string_lossy().into_owned()));
}
