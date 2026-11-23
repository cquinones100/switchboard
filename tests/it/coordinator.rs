//! The coordinator (src/coordinator), as someone sees it: with Settings'
//! `coordinator` on, a Claude in a pane no tab holds, listed in `switchboard
//! tabs` by an address of its own; M-C showing and hiding it beside the tab
//! in front; the events typed into it, held while it works and typed
//! together once it sleeps; and nothing of it with the setting off. Claude
//! is a stand-in that prints how it was started and echoes what is typed.

use crate::common;

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{TestServer, stand_in};
use serde_json::{Value, json};

/// A Claude that says how it was started, then echoes what it is typed.
fn claude(server: &TestServer) {
    stand_in::Claude::new()
        .answer(
            r#"printf 'coordinator up:'
for a in "$@"; do printf ' %s' "$a"; done
printf '\n'
exec cat"#,
        )
        .install(&server.home.join("bin"));
}

/// The coordinator turned on, as Settings turns it on.
fn turn_on(server: &TestServer) {
    common::write_settings(
        &server.home,
        json!({"notifications": false, "coordinator": true}),
    );
}

/// Waits for `pred` to hold of what `read` gives.
fn wait<T: std::fmt::Debug>(what: &str, read: impl Fn() -> T, pred: impl Fn(&T) -> bool) -> T {
    let deadline = common::deadline(Duration::from_secs(20));
    loop {
        let value = read();
        if pred(&value) {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {value:?}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The coordinator's pane, once it runs.
fn coordinator_pane(server: &TestServer) -> String {
    let pane = wait(
        "the coordinator to start",
        || server.list()["coordinator"].clone(),
        Value::is_object,
    );
    pane["id"].as_str().unwrap().to_string()
}

/// Its screen.
fn screen(server: &TestServer, pane: &str) -> String {
    server.ok(&["capture-pane", "-t", pane])
}

/// `switchboard hook <event>` in `pane`, as Claude Code runs it.
fn hook(server: &TestServer, pane: &str, event: &str, input: Value) {
    let mut cmd = Command::new(common::BIN);
    cmd.args(["hook", event])
        .env("HOME", &server.home)
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env("SWITCHBOARD", server.socket())
        .env("SWITCHBOARD_PANE", pane)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (mut ours, theirs) = UnixStream::pair().expect("a socket pair");
    cmd.stdin(Stdio::from(std::os::fd::OwnedFd::from(theirs)));
    let child = cmd.spawn().expect("switchboard runs");
    let _ = ours.write_all(input.to_string().as_bytes());
    drop(ours);
    let out = child.wait_with_output().expect("switchboard ends");
    assert!(out.status.success(), "hook {event}: {out:?}");
}

/// A tab in the background, and its pane.
fn background_tab(server: &TestServer, name: &str) -> String {
    let out = server.ok(&["new-tab", "-d", "-n", name]);
    out.split_whitespace()
        .find(|w| w.starts_with('%'))
        .expect("new-tab prints its pane")
        .to_string()
}

#[test]
fn nothing_of_it_runs_with_the_setting_off() {
    let server = TestServer::start();
    claude(&server);
    common::write_settings(&server.home, json!({"notifications": false}));
    // Long enough for a few of the server's ticks.
    std::thread::sleep(Duration::from_secs(5) * common::patience());
    let list = server.list();
    assert_eq!(list["coordinator"], Value::Null);
    assert!(!server.state.join("coordinator").exists());
    let (_, out, _) = server.run_binary(&["tabs"]);
    assert!(!out.contains("coordinator"), "{out}");
}

#[test]
fn it_runs_on_its_model_in_no_tab_with_an_address_of_its_own() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    claude(&server);
    turn_on(&server);
    let pane = coordinator_pane(&server);
    let started = wait(
        "Claude to start",
        || screen(&server, &pane),
        |s| s.contains("coordinator up:"),
    );
    assert!(started.contains("--model sonnet"), "{started}");
    assert!(
        started.contains("/coordinate"),
        "a fresh session gets its skill: {started}"
    );
    assert!(
        server.state.join("coordinator/settings.json").is_file(),
        "its permissions are a settings file of its own"
    );
    // No tab holds it.
    let list = server.list();
    let in_a_tab = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["tabs"].as_array().unwrap())
        .flat_map(|t| t["panes"].as_array().unwrap())
        .any(|p| p["id"] == pane.as_str());
    assert!(!in_a_tab, "{list}");
    assert_eq!(list["sessions"][0]["tabs"].as_array().unwrap().len(), 1);
    // `switchboard tabs` ends with its line.
    let (_, out, _) = server.run_binary(&["tabs"]);
    let last = out.lines().last().unwrap_or_default();
    assert!(last.starts_with("coordinator\t-\tCoordinator\t"), "{out}");
    // A command it runs names its tab, having none of its own.
    let (code, _, err) = server.typed_in(&pane, &["set", "@tab_name", "x"]);
    assert_ne!(code, 0);
    assert!(err.contains("no tab"), "{err}");
    // Nor can it split its pane, which no tab holds; the server goes on.
    let (code, _, err) = server.typed_in(&pane, &["split"]);
    assert_ne!(code, 0);
    assert!(err.contains("no tab"), "{err}");
    server.list();
}

#[test]
fn its_key_shows_it_beside_the_tab_and_a_second_press_hides_it() {
    let server = TestServer::start();
    claude(&server);
    turn_on(&server);
    let pane = coordinator_pane(&server);
    wait(
        "Claude to start",
        || screen(&server, &pane),
        |s| s.contains("coordinator up:"),
    );
    let mut client = server.attach(160, 40);
    client.wait_text("$");
    client.send(b"\x1bC");
    client.wait_for("its box", |s| {
        s.iter().any(|l| l.contains("Coordinator · asleep"))
    });
    // Its conversation, rewrapped to the box's width.
    client.wait_for("its conversation", |s| {
        s.iter().any(|l| l.contains("/coordinate"))
    });
    // It has the keys: what is typed reaches it.
    client.type_text("hello there");
    wait(
        "the typing in its pane",
        || screen(&server, &pane),
        |s| s.contains("hello there"),
    );
    client.send(b"\x1bC");
    client.wait_for("it to hide", |s| {
        !s.iter().any(|l| l.contains("Coordinator · asleep"))
    });
}

#[test]
fn events_wait_while_it_works_and_are_typed_together_once_it_sleeps() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    claude(&server);
    turn_on(&server);
    let pane = coordinator_pane(&server);
    wait(
        "Claude to start",
        || screen(&server, &pane),
        |s| s.contains("coordinator up:"),
    );
    hook(
        &server,
        &pane,
        "SessionStart",
        json!({"session_id": "coord-1", "source": "startup"}),
    );
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        json!({"session_id": "coord-1", "prompt": "x"}),
    );
    let first = background_tab(&server, "first");
    let second = background_tab(&server, "second");
    for tab in [&first, &second] {
        hook(
            &server,
            tab,
            "UserPromptSubmit",
            json!({"session_id": tab, "prompt": "go"}),
        );
        hook(&server, tab, "Stop", json!({"session_id": tab}));
    }
    // It works: nothing is typed.
    std::thread::sleep(Duration::from_secs(3) * common::patience());
    assert!(!screen(&server, &pane).contains("[switchboard]"));
    // It sleeps: both are typed, in order.
    hook(&server, &pane, "Stop", json!({"session_id": "coord-1"}));
    let typed = wait(
        "the events typed",
        || screen(&server, &pane),
        |s| s.contains("(second) finished"),
    );
    assert!(typed.contains("(first) finished"), "{typed}");
    // Its session's ID is kept for a restart.
    let kept = std::fs::read_to_string(server.state.join("coordinator/state.json")).unwrap();
    assert!(kept.contains("coord-1"), "{kept}");
}

/// Runs git in `dir` with an identity of its own.
#[cfg(feature = "net")]
fn git(dir: &std::path::Path, args: &[&str]) {
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

#[cfg(feature = "net")]
#[test]
fn an_autopilot_tab_s_pr_merging_is_typed_into_it() {
    use common::http::{Fake, Reply};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let merged = Arc::new(AtomicBool::new(false));
    let answer_merged = merged.clone();
    let fake = Fake::github("test-token", move |request| {
        if request.path() != "/graphql" {
            return None;
        }
        let pr = json!({
            "number": 12, "url": "https://github.com/acme/widgets/pull/12",
            "title": "Fix the login",
            "state": if answer_merged.load(Ordering::SeqCst) { "MERGED" } else { "OPEN" },
            "isDraft": false, "baseRefName": "main", "headRefName": "fix-login",
            "headRepositoryOwner": {"login": "acme"}, "reviewDecision": null,
            "additions": 1, "deletions": 0, "mergedAt": null,
            "repository": {"nameWithOwner": "acme/widgets"}, "mergeQueueEntry": null,
            "commits": {"nodes": []}
        });
        let asked = request.text();
        let answer = if asked.contains("pullRequest(number:") {
            json!({"data": {"r0": {"a0": pr}}})
        } else if asked.contains("pullRequests(headRefName") {
            json!({"data": {"repository": {"pullRequests": {"nodes": [pr]}}}})
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
    claude(&server);
    common::write_settings(
        &server.home,
        json!({"notifications": false, "linear": false, "pause_when_away": false,
               "poll_fast": false, "coordinator": true}),
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
    let pane = coordinator_pane(&server);
    hook(
        &server,
        &pane,
        "SessionStart",
        json!({"session_id": "coord-1", "source": "startup"}),
    );
    // Autopilot's tab: autopilot closes it, and the coordinator ticks its item.
    let tab_id = server.list()["sessions"][0]["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["set", "-t", &tab_id, "@autopilot", "1"]);
    let tab = || server.list()["sessions"][0]["tabs"][0]["facts"].clone();
    wait("the tab's open PR", tab, |f| f["pr"]["state"] == "OPEN");

    merged.store(true, Ordering::SeqCst);
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(server.state.join("github"))
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    let typed = wait(
        "the merge typed into it",
        || screen(&server, &pane),
        |s| s.contains("PR #12 merged"),
    );
    assert!(
        typed.contains("https://github.com/acme/widgets/pull/12"),
        "{typed}"
    );
}

#[test]
fn coordinator_order_puts_next_s_issues_first_in_autopilot_s_queue_in_its_order() {
    let server = TestServer::start();
    // Next is A (#3) then B (#1): given priority 1 from the bottom up, as
    // the skill says, and C (#2), which left Next, given 4.
    server.ok(&["coordinator-ordered", "o/r#2", "1", "o/r#2: C"]);
    server.ok(&["coordinator-ordered", "o/r#1", "1", "o/r#1: B"]);
    server.ok(&["coordinator-ordered", "o/r#3", "1", "o/r#3: A"]);
    server.ok(&["coordinator-ordered", "o/r#2", "4", "o/r#2: C"]);
    let mode: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(server.state.join("autopilot.json")).unwrap(),
    )
    .unwrap();
    let next: Vec<&str> = mode["next"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|x| x["text"].as_str())
        .collect();
    assert_eq!(next, ["o/r#3: A", "o/r#1: B"]);
    let (code, _, err) = server.run(&["coordinator-ordered", "o/r#2", "1", "o/r#5: X"]);
    assert_ne!(code, 0, "a first line naming another issue is refused: {err}");
}
