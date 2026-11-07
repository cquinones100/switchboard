//! What the sidebar and mission control learn from GitHub, as someone sees
//! it: a tab in a repository whose branch has a pull request gets that PR's
//! state, merge-queue place, checks and review in its facts, mission
//! control's PRs and Merge queues views fill in, and appending to the
//! `github` file in the state directory has the PR asked about again at
//! once rather than on its next round. GitHub is a stand-in on 127.0.0.1.

#![cfg(feature = "net")]

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::TestServer;
use serde_json::{Value, json};

/// Runs git in `dir` with an identity of its own.
fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// The PR as GitHub's GraphQL gives it, open and queued, or merged.
fn pr(merged: bool) -> Value {
    json!({
        "number": 12, "url": "https://github.com/acme/widgets/pull/12",
        "title": "Fix the login", "state": if merged { "MERGED" } else { "OPEN" },
        "isDraft": false, "baseRefName": "main", "headRefName": "fix-login",
        "headRepositoryOwner": {"login": "acme"}, "reviewDecision": "APPROVED",
        "additions": 30, "deletions": 4, "mergedAt": null,
        "repository": {"nameWithOwner": "acme/widgets"},
        "mergeQueueEntry": if merged { Value::Null } else { json!({"position": 2}) },
        "commits": {"nodes": [{"commit": {"statusCheckRollup": {
            "state": "FAILURE",
            "contexts": {
                "checkRunCountsByState": [{"state": "SUCCESS", "count": 3}, {"state": "FAILURE", "count": 1}],
                "statusContextCountsByState": [],
                "nodes": []
            }
        }}}]}
    })
}

/// A stand-in for GitHub's GraphQL endpoint, answering by what it is asked:
/// the tabs' PRs, the branch's PR (for the tab's name), the searches, the
/// merge queues. `merged` turns the PR merged.
fn fake_github(merged: Arc<AtomicBool>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let mut reader = BufReader::new(stream);
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let asked = String::from_utf8_lossy(&body);
            let pr = pr(merged.load(Ordering::SeqCst));
            let answer = if asked.contains("pullRequest(number:") {
                json!({"data": {"r0": {"a0": pr}}})
            } else if asked.contains("pullRequests(headRefName") {
                json!({"data": {"repository": {"pullRequests": {"nodes": [pr]}}}})
            } else if asked.contains("search(") {
                json!({"data": {"open": {"nodes": [pr]}, "merged": {"nodes": []}, "search": {"nodes": []}}})
            } else if asked.contains("mergeQueue {") {
                json!({"data": {"r0": {"mergeQueue": {
                    "url": "https://github.com/acme/widgets/queue/main",
                    "entries": {"nodes": [{
                        "position": 2, "state": "AWAITING_CHECKS",
                        "enqueuedAt": "2026-11-03T10:00:00Z", "estimatedTimeToMerge": 600,
                        "pullRequest": {"number": 12, "title": "Fix the login",
                            "url": "https://github.com/acme/widgets/pull/12", "author": {"login": "me"}}
                    }]}
                }}}})
            } else {
                json!({"data": {}})
            }
            .to_string();
            let mut stream = reader.into_inner();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            );
        }
    });
    url
}

/// Waits for `pred` to hold of what `read` gives.
fn wait(what: &str, read: impl Fn() -> Value, pred: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let value = read();
        if pred(&value) {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {value}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn a_tab_s_pr_facts_and_the_views_arrive_from_the_background() {
    let merged = Arc::new(AtomicBool::new(false));
    let url = fake_github(merged.clone());
    let server = TestServer::start_with_env(&[
        ("GH_TOKEN", "test-token"),
        ("SWITCHBOARD_GITHUB_GRAPHQL", &url),
        ("SWITCHBOARD_GITHUB_API", &url),
    ]);
    // poll_fast off, so the PR is asked about again within its minute only
    // when something asks for it.
    let dir = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("settings.json"),
        json!({"notifications": false, "linear": false, "pause_when_away": false, "poll_fast": false})
            .to_string(),
    )
    .unwrap();
    let work = server.dir.path().join("widgets");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q"]);
    git(&work, &["symbolic-ref", "HEAD", "refs/heads/fix-login"]);
    git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
    git(
        &work,
        &["remote", "add", "origin", "git@github.com:acme/widgets.git"],
    );

    let mut client = server.attach(100, 30);
    client.wait_text("$");
    client.type_text(&format!("cd {}\r", work.display()));

    let tab = || server.list()["sessions"][0]["tabs"][0].clone();
    let facts = wait(
        "the tab's PR facts",
        || tab()["facts"].clone(),
        |f| f["pr"]["number"] == 12,
    );
    let pr = &facts["pr"];
    assert_eq!(pr["repo"], "acme/widgets");
    assert_eq!(pr["state"], "OPEN");
    assert_eq!(pr["merge"], json!({"queued": {"position": 2}}));
    assert_eq!(pr["checks"], json!({"pass": 3, "fail": 1, "pending": 0}));
    assert_eq!(pr["review_decision"], "APPROVED");

    // Mission control's views, and none for Linear, which is off.
    let sources = || serde_json::from_str::<Value>(&server.ok(&["sources"])).unwrap();
    let views = wait("the PR board and the merge queue", sources, |s| {
        s["views"]["prs"]["items"][0]["title"] == "#12 Fix the login"
            && s["views"]["landing"]["items"][1]["title"].is_string()
    });
    assert_eq!(views["views"]["prs"]["items"][0]["group"], "In merge queue");
    // The Landing view, while the PR is in the merge queue: a section for
    // the repository's merge queue, in order.
    assert_eq!(
        views["views"]["landing"]["items"][0]["title"],
        "acme/widgets · merge queue"
    );
    assert_eq!(
        views["views"]["landing"]["items"][1]["detail"],
        "widgets · awaiting checks · merges in ~10m · me · tab 1"
    );
    // The tab whose PR is queued lands, by its place.
    let facts = wait(
        "the tab's place in the merge queue",
        || tab()["facts"].clone(),
        |f| !f["landing"].is_null(),
    );
    assert_eq!(
        facts["landing"],
        json!({"holding": false, "line": 2, "method": "pr"})
    );
    assert_eq!(views["views"]["tickets"], Value::Null);
    assert_eq!(views["views"]["inbox"], Value::Null);

    // It merges; something that just asked GitHub says so in the github
    // file, and the tab learns it well before its next round.
    merged.store(true, Ordering::SeqCst);
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(server.state.join("github"))
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    let facts = wait(
        "the merge",
        || tab()["facts"].clone(),
        |f| f["pr"]["state"] == "MERGED",
    );
    assert_eq!(facts["pr"]["merge"], "merged");
    assert_eq!(facts["pr"]["checks"], Value::Null);
}
