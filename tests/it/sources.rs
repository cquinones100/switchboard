//! What the sidebar and mission control learn from GitHub, as someone sees
//! it: a tab in a repository whose branch has a pull request gets that PR's
//! state, merge-queue place, checks and review in its facts, mission
//! control's PRs and Merge queues views fill in, and appending to the
//! `github` file in the state directory has the PR asked about again at
//! once rather than on its next round; keys typed in a client keep the
//! lookups going while the computer's own keyboard sits idle and its screen
//! locked, as when the workspace is used over SSH. GitHub is a stand-in on
//! 127.0.0.1.

#![cfg(feature = "net")]

use crate::common;

use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::TestServer;
use common::http::{Fake, Reply};
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

/// A stand-in for GitHub (tests/common/http.rs), as strict as GitHub about
/// how it is asked, its GraphQL answering by what it is asked: the tabs'
/// PRs, the branch's PR (for the tab's name), the searches, the merge
/// queues. `merged` turns the PR merged.
fn fake_github(merged: Arc<AtomicBool>) -> Fake {
    Fake::github("test-token", move |request| {
        if request.path() != "/graphql" {
            return None;
        }
        let asked = request.text();
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
        };
        Some(Reply::json(200, answer))
    })
}

/// Waits for `pred` to hold of what `read` gives.
fn wait(what: &str, read: impl Fn() -> Value, pred: impl Fn(&Value) -> bool) -> Value {
    let deadline = common::deadline(std::time::Duration::from_secs(20));
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
    let fake = fake_github(merged.clone());
    let server = TestServer::start_with_env(&[
        ("GH_TOKEN", "test-token"),
        ("SWITCHBOARD_GITHUB_GRAPHQL", &fake.graphql_url()),
        ("SWITCHBOARD_GITHUB_API", &fake.url),
    ]);
    // poll_fast off, so the PR is asked about again within its minute only
    // when something asks for it.
    common::write_settings(
        &server.home,
        json!({"notifications": false, "linear": false, "pause_when_away": false, "poll_fast": false}),
    );
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

/// A PR from `owner`'s `feature` into `base`, as GitHub's GraphQL gives it.
fn fork_pr(number: u64, base: &str, owner: &str) -> Value {
    json!({
        "number": number, "url": format!("https://github.com/{base}/pull/{number}"),
        "title": format!("Feature {number}"), "state": "OPEN",
        "isDraft": false, "baseRefName": "main", "headRefName": "feature",
        "isCrossRepository": !base.starts_with(&format!("{owner}/")),
        "headRepositoryOwner": {"login": owner}, "reviewDecision": "REVIEW_REQUIRED",
        "additions": 5, "deletions": 1, "mergedAt": null,
        "repository": {"nameWithOwner": base}, "mergeQueueEntry": null,
        "commits": {"nodes": [{"commit": {"statusCheckRollup": {
            "state": "SUCCESS",
            "contexts": {
                "checkRunCountsByState": [{"state": "SUCCESS", "count": 2}],
                "statusContextCountsByState": [],
                "nodes": []
            }
        }}}]}
    })
}

#[test]
fn a_fork_s_tab_gets_its_pr_from_the_project_upstream_names() {
    // GitHub as a fork's clone sees it: origin me/widgets has no PR from
    // `feature`, the project acme/widgets has this fork's (#41) and another
    // fork's of the same name (#52, newer).
    let fake = Fake::github("test-token", move |request| {
        if request.path() != "/graphql" {
            return None;
        }
        let asked = request.text();
        let answer = if asked.contains("pullRequests(headRefName") {
            json!({"data": {
                "repository": {"pullRequests": {"nodes": []}},
                "upstream": {"pullRequests": {"nodes": [
                    fork_pr(52, "acme/widgets", "someone"),
                    fork_pr(41, "acme/widgets", "me"),
                ]}}
            }})
        } else if asked.contains("pullRequest(number: 41)")
            && asked.contains(r#"repository(owner: \"acme\", name: \"widgets\")"#)
        {
            json!({"data": {"r0": {"a0": fork_pr(41, "acme/widgets", "me")}}})
        } else if asked.contains("search(") {
            // Your PRs are searched for in origin and upstream alike.
            let open = request.json()["variables"]["open"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let nodes = if open.contains("repo:me/widgets") && open.contains("repo:acme/widgets") {
                vec![fork_pr(41, "acme/widgets", "me")]
            } else {
                Vec::new()
            };
            json!({"data": {"open": {"nodes": nodes}, "merged": {"nodes": []}}})
        } else {
            json!({"data": {}})
        };
        Some(Reply::json(200, answer))
    });
    let server = TestServer::start_with_env(&[
        ("GH_TOKEN", "test-token"),
        ("SWITCHBOARD_GITHUB_GRAPHQL", &fake.graphql_url()),
        ("SWITCHBOARD_GITHUB_API", &fake.url),
    ]);
    common::write_settings(
        &server.home,
        json!({"notifications": false, "linear": false, "pause_when_away": false}),
    );
    let work = server.dir.path().join("widgets");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q"]);
    git(&work, &["symbolic-ref", "HEAD", "refs/heads/feature"]);
    git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
    git(
        &work,
        &["remote", "add", "origin", "git@github.com:me/widgets.git"],
    );
    git(
        &work,
        &[
            "remote",
            "add",
            "upstream",
            "https://github.com/acme/widgets.git",
        ],
    );
    git(&work, &["config", "branch.feature.remote", "origin"]);
    git(
        &work,
        &["config", "branch.feature.merge", "refs/heads/feature"],
    );

    let mut client = server.attach(100, 30);
    client.wait_text("$");
    client.type_text(&format!("cd {}\r", work.display()));

    let tab = || server.list()["sessions"][0]["tabs"][0].clone();
    let facts = wait(
        "the tab's PR facts, from upstream",
        || tab()["facts"].clone(),
        |f| f["pr"]["number"] == 41,
    );
    assert_eq!(facts["pr"]["repo"], "acme/widgets");
    assert_eq!(
        facts["pr"]["url"],
        "https://github.com/acme/widgets/pull/41"
    );
    assert_eq!(
        facts["pr"]["checks"],
        json!({"pass": 2, "fail": 0, "pending": 0})
    );
    assert_eq!(tab()["pr"], 41);
    assert!(
        tab()["label"].as_str().unwrap().starts_with("#41 "),
        "{}",
        tab()["label"]
    );

    // The PR board has it, from the search over both repositories.
    let sources = || serde_json::from_str::<Value>(&server.ok(&["sources"])).unwrap();
    let views = wait("the PR board", sources, |s| {
        s["views"]["prs"]["items"][0]["title"] == "#41 Feature 41"
    });
    assert_eq!(
        views["views"]["prs"]["items"][0]["data"]["pr"]["repo"],
        "acme/widgets"
    );
}

#[test]
fn keys_typed_from_another_computer_keep_the_lookups_going_while_it_sits_idle_and_locked() {
    let fake = fake_github(Arc::new(AtomicBool::new(false)));
    // The computer's own keyboard and mouse untouched for three days and
    // its screen locked, as a Mac's are while the workspace is used over
    // SSH; "Pause lookups while away" on, as it is unless turned off.
    let server = TestServer::start_with_env_and_settings(
        &[
            ("GH_TOKEN", "test-token"),
            ("SWITCHBOARD_GITHUB_GRAPHQL", &fake.graphql_url()),
            ("SWITCHBOARD_GITHUB_API", &fake.url),
            ("SWITCHBOARD_PRESENCE", "259200000,locked"),
        ],
        &json!({"notifications": false, "linear": false, "poll_fast": false}).to_string(),
    );
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
    // Every lookup's first ask goes ahead away or not; the PR board's comes
    // after the tabs' PRs' (src/sources/schedule.rs), so once it has
    // answered, a lookup asks again only while you're here.
    let sources = || serde_json::from_str::<Value>(&server.ok(&["sources"])).unwrap();
    wait("the first round of lookups", sources, |s| {
        !s["views"]["prs"].is_null()
    });

    // Keys typed in the client, after the lock was seen: the tab's new PR
    // is asked about at once, as it is while you're at the computer.
    client.type_text(&format!("cd {}\r", work.display()));
    let tab = || server.list()["sessions"][0]["tabs"][0].clone();
    let facts = wait(
        "the tab's PR facts",
        || tab()["facts"].clone(),
        |f| f["pr"]["number"] == 12,
    );
    assert_eq!(
        facts["pr"]["checks"],
        json!({"pass": 3, "fail": 1, "pending": 0})
    );
}

#[test]
fn the_description_beside_the_pr_board_draws_its_diagram_as_a_picture_where_the_client_draws_images()
 {
    const BODY: &str = "Why\n\n```mermaid\nflowchart LR\n  A[Login] --> B[Token]\n```\n";
    let fake = Fake::github("test-token", |request| {
        if request.path() != "/graphql" {
            return None;
        }
        let mut pr = pr(false);
        pr["body"] = json!(BODY);
        let answer = if request.text().contains("search(") {
            json!({"data": {"open": {"nodes": [pr]}, "merged": {"nodes": []}, "search": {"nodes": []}}})
        } else {
            json!({"data": {}})
        };
        Some(Reply::json(200, answer))
    });
    let server = TestServer::start_with_env_and_settings(
        &[
            ("GH_TOKEN", "test-token"),
            ("SWITCHBOARD_GITHUB_GRAPHQL", &fake.graphql_url()),
            ("SWITCHBOARD_GITHUB_API", &fake.url),
        ],
        &json!({"notifications": false, "linear": false, "pause_when_away": false, "ux2": true})
            .to_string(),
    );
    // The diagram as Mermaid would have drawn it, kept in the image cache
    // under its source and theme, so no Chrome runs.
    let source = switchboard::mission::diagrams::sources(BODY, "").remove(0);
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(400, 200, image::Rgba([200, 60, 60, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let cache = switchboard::views::image::cache::Cache::at(server.home.join(".cache/switchboard"));
    for light in [false, true] {
        let key = switchboard::views::pictures::diagram_key(&source, light);
        cache.keep_asset(&key, "png", &png).expect("kept");
    }
    let work = server.dir.path().join("widgets");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q"]);
    git(&work, &["symbolic-ref", "HEAD", "refs/heads/fix-login"]);
    git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
    git(
        &work,
        &["remote", "add", "origin", "git@github.com:acme/widgets.git"],
    );
    let sources = || serde_json::from_str::<Value>(&server.ok(&["sources"])).unwrap();
    let placeholder = '\u{10EEEE}';

    // A terminal that draws the server's images: the picture is sent, its
    // placeholders are in the pane beside the board, and the text drawing
    // is gone.
    {
        let mut ghostty = server.attach_as("xterm-ghostty", 200, 40);
        ghostty.wait_text("$");
        ghostty.type_text(&format!("cd {}\r", work.display()));
        wait("the PR board", sources, |s| {
            s["views"]["prs"]["items"][0]["title"] == "#12 Fix the login"
        });
        server.ok(&["press", "mission-prs"]);
        ghostty.wait_text("Why");
        let screen = ghostty.wait_for("the diagram's picture", |s| {
            s.iter().any(|l| l.contains(placeholder))
        });
        assert!(!screen.iter().any(|l| l.contains("Login")), "{screen:#?}");
        let (_, col) = ghostty.find_cell(placeholder).expect("its first cell");
        assert!(col >= 150, "in the pane beside the board: column {col}");
        assert!(
            common::sent(&ghostty.raw, b"\x1b_Ga=T,U=1,f=100,t=d,q=2,i="),
            "the picture sent inline"
        );
    }

    // One that doesn't: the diagram stays its text drawing.
    let mut plain = server.attach_as("xterm-256color", 200, 40);
    plain.wait_text("$");
    server.ok(&["press", "mission-prs"]);
    let screen = plain.wait_text("Login");
    assert!(!screen.iter().any(|l| l.contains(placeholder)));
    assert!(!common::sent(&plain.raw, b"\x1b_Ga=T"));
}

#[test]
fn j_and_k_scroll_the_description_beside_the_pr_board() {
    let body: String = (1..=30)
        .map(|i| format!("Line {i}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    let fake = Fake::github("test-token", move |request| {
        if request.path() != "/graphql" {
            return None;
        }
        let mut pr = pr(false);
        pr["body"] = json!(body);
        let answer = if request.text().contains("search(") {
            json!({"data": {"open": {"nodes": [pr]}, "merged": {"nodes": []}, "search": {"nodes": []}}})
        } else {
            json!({"data": {}})
        };
        Some(Reply::json(200, answer))
    });
    let server = TestServer::start_with_env_and_settings(
        &[
            ("GH_TOKEN", "test-token"),
            ("SWITCHBOARD_GITHUB_GRAPHQL", &fake.graphql_url()),
            ("SWITCHBOARD_GITHUB_API", &fake.url),
        ],
        &json!({"notifications": false, "linear": false, "pause_when_away": false, "ux2": true})
            .to_string(),
    );
    let work = server.dir.path().join("widgets");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q"]);
    git(&work, &["symbolic-ref", "HEAD", "refs/heads/fix-login"]);
    git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
    git(
        &work,
        &["remote", "add", "origin", "git@github.com:acme/widgets.git"],
    );
    let sources = || serde_json::from_str::<Value>(&server.ok(&["sources"])).unwrap();
    let mut client = server.attach(200, 30);
    client.wait_text("$");
    client.type_text(&format!("cd {}\r", work.display()));
    wait("the PR board", sources, |s| {
        s["views"]["prs"]["items"][0]["title"] == "#12 Fix the login"
    });
    server.ok(&["press", "mission-prs"]);
    let screen = client.wait_text("Line 1");
    assert!(!screen.iter().any(|l| l.contains("Line 30")), "{screen:#?}");
    assert!(
        screen.iter().any(|l| l.contains("J K scroll")),
        "{screen:#?}"
    );

    // J to the end of the description, which stops there; K back to its top.
    client.type_text(&"J".repeat(80));
    let screen = client.wait_text("Line 30");
    let first_line = |l: &String| l.trim_end().ends_with("Line 1");
    assert!(!screen.iter().any(first_line), "{screen:#?}");
    client.type_text(&"K".repeat(80));
    client.wait_for("the description from its top", |s| {
        !s.iter().any(|l| l.contains("Line 30")) && s.iter().any(first_line)
    });
}
