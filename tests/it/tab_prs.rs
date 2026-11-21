//! A tab follows every PR its Claude session opens (src/tabs/prs.rs): a
//! stand-in Claude's hooks report two PRs opened from other branches and
//! another repository, and the tab lists them beside its branch's, its
//! label leading with the current one, the sidebar showing how many more;
//! "Show another of this tab's PRs" makes another current, which M-v then
//! opens; a merged one leaves the list once the tab has been in front; the
//! list survives a restart; and with Settings' `tab_prs` off only the
//! branch's PR shows. GitHub is a stand-in on 127.0.0.1.

use crate::common;

use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::TestServer;
use common::http::{Fake, Reply};
use serde_json::{Value, json};
use switchboard::mux::proto::ClientMsg;

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

/// Each PR's state by `owner/name#n`, which the test changes.
type States = Arc<Mutex<std::collections::HashMap<String, &'static str>>>;

/// GitHub: the branch `web-142`'s PR is acme/web-app#2040; every PR by its
/// number, in whichever repository the query names, in the state `states`
/// gives it.
fn github(states: States) -> Fake {
    Fake::github("test-token", move |request| {
        if request.path() != "/graphql" {
            return None;
        }
        let body = request.json();
        let text = body["query"].as_str().unwrap_or("").to_string();
        let states = states.lock().unwrap().clone();
        let pr = |repo: &str, number: u64| {
            let key = format!("{repo}#{number}");
            json!({
                "number": number, "url": format!("https://github.com/{repo}/pull/{number}"),
                "title": format!("Title of {key}"), "state": states.get(&key).copied().unwrap_or("OPEN"),
                "isDraft": false, "baseRefName": "main", "headRefName": format!("head-{number}"),
                "headRepositoryOwner": {"login": "acme"}, "isCrossRepository": false,
                "mergeQueueEntry": null
            })
        };
        if let Some(head) = body["variables"]["head"].as_str() {
            let nodes: Vec<Value> = match head {
                "web-142" => vec![pr("acme/web-app", 2040)],
                _ => vec![],
            };
            return Some(Reply::json(
                200,
                json!({"data": {"repository": {"pullRequests": {"nodes": nodes}}}}),
            ));
        }
        if let Some(number) = body["variables"]["number"].as_u64() {
            // The PR view's own read, by its number.
            let repo = format!(
                "{}/{}",
                body["variables"]["owner"].as_str().unwrap_or(""),
                body["variables"]["name"].as_str().unwrap_or("")
            );
            return Some(Reply::json(
                200,
                json!({"data": {"repository": {"pullRequest": pr(&repo, number)}}}),
            ));
        }
        // The sidebar's query: each repository's PRs by number.
        let repos = regex::Regex::new(r#"(r\d+): repository\(owner: "([^"]+)", name: "([^"]+)"\)"#)
            .unwrap();
        let numbers = regex::Regex::new(r"(a\d+): pullRequest\(number: (\d+)\)").unwrap();
        let starts: Vec<(usize, String, String)> = repos
            .captures_iter(&text)
            .map(|c| {
                (
                    c.get(0).unwrap().start(),
                    c[1].to_string(),
                    format!("{}/{}", &c[2], &c[3]),
                )
            })
            .collect();
        let mut data = serde_json::Map::new();
        for c in numbers.captures_iter(&text) {
            let at = c.get(0).unwrap().start();
            let Some((_, alias, repo)) = starts.iter().rev().find(|(s, ..)| *s < at) else {
                continue;
            };
            let entry = data.entry(alias.clone()).or_insert_with(|| json!({}));
            entry[&c[1]] = pr(repo, c[2].parse().unwrap());
        }
        Some(Reply::json(200, json!({"data": data})))
    })
}

fn start(github: &Fake) -> TestServer {
    let server = TestServer::start_with_env(&[
        ("GH_TOKEN", "test-token"),
        ("SWITCHBOARD_GITHUB_GRAPHQL", &github.graphql_url()),
        ("SWITCHBOARD_GITHUB_API", &github.url),
    ]);
    common::write_settings(
        &server.home,
        json!({"notifications": false, "linear": false, "ux2": true}),
    );
    server
}

/// A clone of acme/web-app on `web-142`.
fn clone(server: &TestServer) -> std::path::PathBuf {
    let dir = server.dir.path().join("web-app");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    git(&dir, &["symbolic-ref", "HEAD", "refs/heads/web-142"]);
    git(&dir, &["commit", "-q", "--allow-empty", "-m", "first"]);
    git(
        &dir,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/web-app.git",
        ],
    );
    dir
}

fn tab(server: &TestServer) -> Value {
    server.list()["sessions"][0]["tabs"][0].clone()
}

fn wait_tab(server: &TestServer, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let deadline = common::deadline(Duration::from_secs(20));
    loop {
        let t = tab(server);
        if pred(&t) {
            return t;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {t}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn prs(t: &Value) -> Vec<String> {
    t["prs"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Claude Code's PostToolUse for a Bash call, sent as `switchboard hook`
/// sends it, from `pane`.
fn bash_ended(server: &TestServer, pane: &str, command: &str, stdout: &str) {
    let payload = json!({
        "hook_event_name": "PostToolUse", "session_id": "s1", "tool_name": "Bash",
        "tool_use_id": format!("call-{}", stdout.len()),
        "tool_input": {"command": command},
        "tool_response": {"stdout": stdout, "stderr": "", "interrupted": false},
    });
    let msg = ClientMsg::Command {
        args: vec!["hook".into(), "PostToolUse".into()],
        cwd: server.dir.path().to_path_buf(),
        pane: Some(pane.to_string()),
        stdin: Some(payload.to_string().into_bytes()),
        by_hand: false,
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = switchboard::mux::socket::run_command_at(&server.socket(), &msg, &mut out, &mut err);
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
}

#[test]
fn a_tab_follows_every_pr_its_session_opens() {
    let states: States = Arc::default();
    let github = github(states.clone());
    let mut server = start(&github);
    let dir = clone(&server);
    let mut client = server.attach(160, 40);
    client.wait_text("$");
    client.type_text(&format!("cd {}\r", dir.display()));
    wait_tab(&server, "the branch's PR", |t| t["pr"] == 2040);
    let pane = tab(&server)["panes"][0]["id"].as_str().unwrap().to_string();

    // The stack's base, then a PR in another repository; one only echoed
    // is no PR the session opened.
    bash_ended(
        &server,
        &pane,
        "gh pr create --base main --head web-140",
        "https://github.com/acme/web-app/pull/2024\n",
    );
    bash_ended(
        &server,
        &pane,
        "cd ~/code/oncall-tools && gh pr create --draft",
        "https://github.com/acme/oncall-tools/pull/318\n",
    );
    bash_ended(
        &server,
        &pane,
        "echo https://github.com/acme/x/pull/9",
        "https://github.com/acme/x/pull/9",
    );
    let t = wait_tab(&server, "both PRs listed", |t| prs(t).len() == 3);
    assert_eq!(
        prs(&t),
        [
            "acme/web-app#2040",
            "acme/web-app#2024",
            "acme/oncall-tools#318"
        ]
    );
    // The branch's PR is current: the label leads with it.
    assert_eq!(t["pr"], 2040);
    assert!(t["label"].as_str().unwrap().starts_with("#2040 "), "{t}");
    client.wait_for("the sidebar's count of the others", |s| {
        s.iter().any(|l| l.contains("#2040") && l.contains("+2"))
    });

    // The picker makes another current, which the PR view open in the tab
    // opens again on, by its number.
    server.ok(&["press", "pr-view"]);
    server.ok(&["press", "tab-prs"]);
    client.wait_text("Show another of this tab's PRs");
    client.type_text("318");
    client.wait_for("the picker filtered to #318", |s| {
        !s.iter().any(|l| l.contains("#2024  web-app"))
    });
    client.type_text("\r");
    let t = wait_tab(&server, "#318 current", |t| t["pr"] == 318);
    assert!(t["label"].as_str().unwrap().starts_with("#318 "), "{t}");
    let deadline = common::deadline(Duration::from_secs(20));
    while !github.seen().iter().any(|r| {
        r.json()["variables"]["number"] == 318 && r.json()["variables"]["name"] == "oncall-tools"
    }) {
        assert!(Instant::now() < deadline, "M-v never read #318");
        std::thread::sleep(Duration::from_millis(100));
    }
    server.ok(&["press", "pr-view"]);

    // Merged, it is marked on one answer while the tab is in front, and
    // gone from the list on the next.
    states
        .lock()
        .unwrap()
        .insert("acme/web-app#2024".into(), "MERGED");
    let deadline = common::deadline(Duration::from_secs(30));
    while prs(&tab(&server)).len() == 3 {
        assert!(
            Instant::now() < deadline,
            "the merged PR stayed: {}",
            tab(&server)
        );
        server.ok(&["sources", "refresh"]);
        std::thread::sleep(Duration::from_millis(300));
    }
    assert_eq!(
        prs(&tab(&server)),
        ["acme/web-app#2040", "acme/oncall-tools#318"]
    );

    // The list and the pick survive a restart of the layout.
    drop(client);
    server.ok(&["save"]);
    server.restart();
    let t = wait_tab(&server, "the list restored", |t| {
        prs(t).len() == 2 && t["pr"] == 318
    });
    assert_eq!(prs(&t), ["acme/web-app#2040", "acme/oncall-tools#318"]);

    // Off, only the branch's PR.
    common::write_settings(
        &server.home,
        json!({"notifications": false, "linear": false, "ux2": true, "tab_prs": false}),
    );
    let t = wait_tab(&server, "only the branch's PR", |t| prs(t).len() == 1);
    assert_eq!(t["pr"], 2040);
}
