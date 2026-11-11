//! What Claude is doing, as the workspace shows it: `switchboard hook` run as
//! Claude Code's hooks run it, with a pane's SWITCHBOARD and SWITCHBOARD_PANE
//! and the hook's JSON on stdin, against a server of the test's own, and the
//! pane's state read back from `list` and the status line.
//!
//! A pane counts as running Claude when a process in front on its terminal
//! has a session file in `~/.claude/sessions`; the tests write one for the
//! pane's shell, in the test's own home, so the shell stands in for Claude.

use crate::common;

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{TempDir, TestServer, stand_in};
use serde_json::{Value, json};

const ALT: &str = "\x1b";

/// Runs `switchboard hook <event>` with `input` on stdin, in the home and
/// state directory given, inside the pane `pane` of the server at `socket`
/// when both are given: (code, stdout, stderr).
fn run_hook(
    home: &Path,
    state: &Path,
    pane: Option<(&Path, &str)>,
    event: &str,
    input: &str,
) -> (i32, String, String) {
    let mut cmd = Command::new(common::BIN);
    cmd.args(["hook", event])
        .env("HOME", home)
        .env("SWITCHBOARD_STATE_DIR", state)
        .env_remove("SWITCHBOARD")
        .env_remove("SWITCHBOARD_PANE")
        .env_remove("SWITCHBOARD_SETTINGS")
        .env_remove("SLACK_RELAY_HELPER")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some((socket, pane)) = pane {
        cmd.env("SWITCHBOARD", socket).env("SWITCHBOARD_PANE", pane);
    }
    // The JSON on a socket, as Claude Code hands a hook its input, written
    // and closed before the hook runs.
    let (mut ours, theirs) = UnixStream::pair().expect("a socket pair");
    cmd.stdin(Stdio::from(std::os::fd::OwnedFd::from(theirs)));
    let child = cmd.spawn().expect("switchboard runs");
    // A hook that has stopped reading is no reason to fail here.
    let _ = ours.write_all(input.as_bytes());
    drop(ours);
    let out = child.wait_with_output().expect("switchboard ends");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A hook in `pane`, which must succeed and say nothing.
fn hook(server: &TestServer, pane: &str, event: &str, input: Value) {
    let got = run_hook(
        &server.home,
        &server.state,
        Some((&server.socket(), pane)),
        event,
        &input.to_string(),
    );
    assert_eq!(
        got,
        (0, String::new(), String::new()),
        "hook {event}; server log: {}",
        server.log()
    );
}

/// Settings for the tests: no desktop notifications from them.
fn settings(server: &TestServer, extra: Value) {
    let mut all = json!({"notifications": false});
    for (k, v) in extra.as_object().unwrap() {
        all[k] = v.clone();
    }
    common::write_settings(&server.home, all);
}

/// Writes a Claude session file for `pid`, as Claude Code does for each of
/// its processes.
fn session_file(home: &Path, pid: u64, session: Value) {
    let dir = home.join(".claude/sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let mut file = json!({"pid": pid, "entrypoint": "cli"});
    for (k, v) in session.as_object().unwrap() {
        file[k] = v.clone();
    }
    std::fs::write(dir.join(format!("{pid}.json")), file.to_string()).unwrap();
}

/// The first session's tabs.
fn tabs(list: &Value) -> Vec<Value> {
    list["sessions"][0]["tabs"].as_array().unwrap().clone()
}

/// A pane's entry in `list`, by id.
fn pane_of(list: &Value, id: &str) -> Value {
    tabs(list)
        .iter()
        .flat_map(|t| t["panes"].as_array().unwrap().clone())
        .find(|p| p["id"] == id)
        .unwrap_or_else(|| panic!("no pane {id} in {list}"))
}

/// The first tab's first pane: its id and its shell's pid.
fn first_pane(server: &TestServer) -> (String, u64) {
    let pane = tabs(&server.list())[0]["panes"][0].clone();
    (
        pane["id"].as_str().unwrap().to_string(),
        pane["pid"].as_u64().unwrap(),
    )
}

/// Waits until `list` satisfies `pred`.
fn wait_list(server: &TestServer, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    loop {
        let list = server.list();
        if pred(&list) {
            return list;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {list}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_hook_marks_its_pane_and_the_status_line_shows_it() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "s-1"}));

    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        json!({"hook_event_name": "UserPromptSubmit", "session_id": "s-1", "prompt": "/review-loop now"}),
    );
    let agent = pane_of(&server.list(), &pane)["agent"].clone();
    assert_eq!(agent["state"], "working");
    assert_eq!(agent["loop"], "review");
    assert!(agent["since"].as_i64().unwrap() > 0);
    client.wait_for("the working badge", |s| s[29].contains("1:  ⋯"));

    hook(
        &server,
        &pane,
        "PreToolUse",
        json!({"tool_name": "Bash", "tool_input": {"command": "cargo test", "description": "Run the tests"}}),
    );
    assert_eq!(
        pane_of(&server.list(), &pane)["agent"]["activity"],
        "Run the tests"
    );

    // Finishing in the tab you're looking at isn't news, so look elsewhere.
    server.ok(&["new-tab", "-n", "other"]);
    client.wait_text("2:other");
    hook(&server, &pane, "Stop", json!({"session_id": "s-1"}));
    let list = server.list();
    assert_eq!(pane_of(&list, &pane)["agent"]["state"], "done");
    assert_eq!(tabs(&list)[0]["agent"], "done");
    let screen = client.wait_for("the done badge and count", |s| {
        s[29].contains("1:  ✓") && s[29].contains("✓ 1")
    });
    assert!(!screen[29].contains('⋯'), "{:?}", screen[29]);

    hook(
        &server,
        &pane,
        "Notification",
        json!({"notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash"}),
    );
    assert_eq!(pane_of(&server.list(), &pane)["agent"]["state"], "waiting");
    assert_eq!(
        server.ok(&["show", "-t", &pane, "@agent_state"]).trim(),
        "waiting"
    );
    client.wait_for("the waiting badge", |s| {
        s[29].contains("1:  !") && s[29].contains("! 1")
    });

    hook(&server, &pane, "SessionEnd", json!({"reason": "exit"}));
    let list = server.list();
    assert_eq!(pane_of(&list, &pane)["agent"], Value::Null);
    assert_eq!(tabs(&list)[0]["agent"], Value::Null);
    client.wait_for("no badge", |s| !s[29].contains('!'));
}

#[test]
fn a_finished_tab_clears_once_it_has_been_in_front() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "s-1"}));
    let first = tabs(&server.list())[0]["id"].as_str().unwrap().to_string();
    server.ok(&["new-tab", "-n", "other"]);
    client.wait_text("2:other");
    hook(&server, &pane, "UserPromptSubmit", json!({}));
    hook(&server, &pane, "Stop", json!({}));
    assert_eq!(pane_of(&server.list(), &pane)["agent"]["state"], "done");
    server.ok(&["select-tab", "-t", &first]);
    wait_list(&server, "the finished mark seen", |l| {
        pane_of(l, &pane)["agent"].is_null()
    });

    // With done_clears at "prompt", it stays until the next prompt.
    settings(&server, json!({"done_clears": "prompt"}));
    let second = tabs(&server.list())[1]["id"].as_str().unwrap().to_string();
    server.ok(&["select-tab", "-t", &second]);
    hook(&server, &pane, "Stop", json!({}));
    server.ok(&["select-tab", "-t", &first]);
    std::thread::sleep(Duration::from_millis(1500) * common::patience());
    assert_eq!(pane_of(&server.list(), &pane)["agent"]["state"], "done");
    hook(&server, &pane, "UserPromptSubmit", json!({}));
    assert_eq!(pane_of(&server.list(), &pane)["agent"]["state"], "working");
}

#[test]
fn a_claude_parked_behind_a_tool_still_marks_its_tab_and_is_seen_there() {
    let server = TestServer::start();
    settings(&server, json!({}));
    // A stand-in lazygit to take the pane's place.
    stand_in::lazygit(&server.home.join("bin"));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "s-1"}));
    let first = tabs(&server.list())[0]["id"].as_str().unwrap().to_string();
    hook(&server, &pane, "UserPromptSubmit", json!({}));
    server.ok(&["press", "lazygit"]);
    wait_list(&server, "lazygit in the pane's place", |l| {
        tabs(l)[0]["panes"][0]["options"]["@in_place"] == "lazygit"
    });
    // The tab still shows what its Claude is doing behind it.
    assert_eq!(tabs(&server.list())[0]["agent"], "working");
    server.ok(&["new-tab", "-n", "other"]);
    client.wait_text("2:other");
    hook(&server, &pane, "Stop", json!({}));
    assert_eq!(tabs(&server.list())[0]["agent"], "done");
    // Looking at the tab sees it finished, though lazygit is in front.
    server.ok(&["select-tab", "-t", &first]);
    wait_list(&server, "the finished mark seen behind lazygit", |l| {
        tabs(l)[0]["agent"].is_null()
    });
}

/// A line of a Claude transcript, said now, as Claude Code appends one.
fn say(file: &Path, entry: Value) {
    let mut entry = entry;
    entry["timestamp"] = json!(jiff::Timestamp::now().to_string());
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .unwrap();
    writeln!(f, "{entry}").unwrap();
}

#[test]
fn a_request_you_denied_hands_the_tab_back_as_idle_a_parked_claude_s_too() {
    let server = TestServer::start();
    settings(&server, json!({}));
    stand_in::lazygit(&server.home.join("bin"));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    session_file(
        &server.home,
        pid,
        json!({"sessionId": "denied", "cwd": "/work"}),
    );
    let dir = server.home.join(".claude/projects/-work");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("denied.jsonl");
    let interrupted = json!({"type": "user", "message": {"content": [{"type": "text", "text": "[Request interrupted by user for tool use]"}]}});
    let request = json!({"tool_name": "Bash", "tool_input": {"command": "git push"}});

    say(
        &file,
        json!({"type": "user", "message": {"content": "push it"}}),
    );
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        json!({"prompt": "push it"}),
    );
    hook(&server, &pane, "PermissionRequest", request.clone());
    let agent = pane_of(&server.list(), &pane)["agent"].clone();
    assert_eq!(agent["state"], "waiting");
    assert_eq!(agent["wait"], "permission: Bash git push");
    // With no interruption, it waits, past the sidebar's next read.
    std::thread::sleep(Duration::from_millis(2_500) * common::patience());
    assert_eq!(pane_of(&server.list(), &pane)["agent"]["state"], "waiting");

    // Denied: no hook runs, but the transcript says so.
    say(&file, interrupted.clone());
    let list = wait_list(&server, "the pane handed back", |l| {
        pane_of(l, &pane)["agent"]["state"].is_null()
    });
    assert_eq!(pane_of(&list, &pane)["agent"]["wait"], Value::Null);
    assert_eq!(tabs(&list)[0]["agent"], Value::Null);

    // Asked again, with lazygit in its place: denied from elsewhere (the
    // Slack relay), the parked Claude is handed back too.
    say(
        &file,
        json!({"type": "user", "message": {"content": "push it to a branch"}}),
    );
    hook(&server, &pane, "UserPromptSubmit", json!({}));
    hook(&server, &pane, "PermissionRequest", request);
    server.ok(&["press", "lazygit"]);
    wait_list(&server, "lazygit in the pane's place", |l| {
        tabs(l)[0]["panes"][0]["options"]["@in_place"] == "lazygit"
    });
    assert_eq!(tabs(&server.list())[0]["agent"], "waiting");
    say(&file, interrupted);
    wait_list(&server, "the parked pane handed back", |l| {
        tabs(l)[0]["agent"].is_null()
    });
}

#[test]
fn a_request_auto_mode_allows_keeps_the_tab_working_until_a_prompt_reaches_you() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "auto"}));
    let test = json!({"tool_name": "Bash", "tool_use_id": "b1", "permission_mode": "auto",
        "tool_input": {"command": "cargo test", "description": "Run the tests"}});
    let state = |server: &TestServer| pane_of(&server.list(), &pane)["agent"].clone();

    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        json!({"prompt": "test it"}),
    );
    for event in ["PreToolUse", "PermissionRequest", "PostToolUse"] {
        hook(&server, &pane, event, test.clone());
        let agent = state(&server);
        assert_eq!(agent["state"], "working", "after {event}");
        assert_eq!(agent["wait"], Value::Null, "after {event}");
    }

    // A prompt that does reach you comes with its Notification.
    hook(&server, &pane, "PreToolUse", test.clone());
    hook(&server, &pane, "PermissionRequest", test.clone());
    hook(
        &server,
        &pane,
        "Notification",
        json!({"notification_type": "permission_prompt", "message": "Claude needs your permission"}),
    );
    let agent = state(&server);
    assert_eq!(agent["state"], "waiting");
    assert_eq!(agent["wait"], "permission: Bash cargo test");
}

#[test]
fn a_permission_request_with_no_tool_takes_the_transcripts_words() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    session_file(
        &server.home,
        pid,
        json!({"sessionId": "unnamed", "cwd": "/work"}),
    );
    let dir = server.home.join(".claude/projects/-work");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("unnamed.jsonl");
    say(
        &file,
        json!({"type": "user", "message": {"content": "wait a bit"}}),
    );
    say(
        &file,
        json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "sleep 30"}}]}}),
    );
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        json!({"prompt": "wait a bit"}),
    );
    hook(&server, &pane, "PermissionRequest", json!({}));
    let list = wait_list(&server, "the wait named from the transcript", |l| {
        pane_of(l, &pane)["agent"]["wait"] == "permission: Bash: sleep 30"
    });
    assert_eq!(pane_of(&list, &pane)["agent"]["state"], "waiting");
}

#[test]
fn work_you_interrupted_hands_the_tab_back_as_idle_until_the_next_prompt() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    session_file(
        &server.home,
        pid,
        json!({"sessionId": "escaped", "cwd": "/work"}),
    );
    let dir = server.home.join(".claude/projects/-work");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("escaped.jsonl");

    say(
        &file,
        json!({"type": "user", "message": {"content": "wait a while"}}),
    );
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        json!({"prompt": "wait a while"}),
    );
    hook(
        &server,
        &pane,
        "PreToolUse",
        json!({"tool_name": "Bash", "tool_input": {"command": "sleep 120"}}),
    );
    assert_eq!(pane_of(&server.list(), &pane)["agent"]["state"], "working");

    // Esc mid-command: no hook runs, but the transcript says so.
    say(
        &file,
        json!({"type": "user", "message": {"content": [{"type": "text", "text": "[Request interrupted by user]"}]}}),
    );
    wait_list(&server, "the pane handed back", |l| {
        pane_of(l, &pane)["agent"]["state"].is_null()
    });

    // A prompt after it works on, the interruption still the transcript's
    // last entry, past the sidebar's next read.
    hook(&server, &pane, "UserPromptSubmit", json!({}));
    assert_eq!(pane_of(&server.list(), &pane)["agent"]["state"], "working");
    std::thread::sleep(Duration::from_millis(2_500) * common::patience());
    assert_eq!(pane_of(&server.list(), &pane)["agent"]["state"], "working");
}

#[test]
fn a_compact_you_typed_that_fails_hands_the_tab_back_as_idle() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    let now_ms = || jiff::Timestamp::now().as_millisecond();
    // Idle, as Claude Code said when the last turn ended.
    let before = now_ms();
    session_file(
        &server.home,
        pid,
        json!({"sessionId": "compacted", "cwd": "/work", "status": "idle", "statusUpdatedAt": before}),
    );
    std::thread::sleep(Duration::from_millis(1_100));
    hook(&server, &pane, "PreCompact", json!({"trigger": "manual"}));
    assert_eq!(pane_of(&server.list(), &pane)["agent"]["state"], "working");
    // Claude Code's word from before it began doesn't end it, past the
    // sidebar's next read.
    std::thread::sleep(Duration::from_millis(2_500) * common::patience());
    assert_eq!(pane_of(&server.list(), &pane)["agent"]["state"], "working");

    // It fails, or you cancel it: no PostCompact runs, but Claude Code says
    // the session is idle again.
    session_file(
        &server.home,
        pid,
        json!({"sessionId": "compacted", "cwd": "/work", "status": "idle", "statusUpdatedAt": now_ms()}),
    );
    wait_list(&server, "the pane handed back", |l| {
        pane_of(l, &pane)["agent"]["state"].is_null()
    });
}

#[test]
fn a_pane_not_running_claude_keeps_no_state() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (pane, _) = first_pane(&server);
    // No session file for the pane's shell: whatever said it was Claude
    // isn't, and nothing is left behind.
    hook(&server, &pane, "UserPromptSubmit", json!({}));
    assert_eq!(pane_of(&server.list(), &pane)["agent"], Value::Null);
}

#[test]
fn a_shell_holding_a_dead_claude_s_pid_drops_its_state() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "s-1"}));
    hook(&server, &pane, "UserPromptSubmit", json!({}));
    assert_eq!(pane_of(&server.list(), &pane)["agent"]["state"], "working");

    // The file now dates from before the shell started: a Claude that had
    // the pid before it ended without saying so, and the shell got its pid.
    let file = server.home.join(format!(".claude/sessions/{pid}.json"));
    let hour_ago = std::time::SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(hour_ago)
        .unwrap();
    let start = Instant::now();
    wait_list(&server, "the working state dropped", |l| {
        pane_of(l, &pane)["agent"].is_null()
    });
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "{:?}",
        start.elapsed()
    );
    assert!(file.exists());
}

#[test]
fn a_headless_claudes_hooks_mark_nothing() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "tab"}));
    session_file(
        &server.home,
        999_998,
        json!({"sessionId": "loop", "entrypoint": "sdk-cli"}),
    );
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        json!({"session_id": "loop"}),
    );
    assert_eq!(pane_of(&server.list(), &pane)["agent"], Value::Null);
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        json!({"session_id": "tab"}),
    );
    assert_eq!(pane_of(&server.list(), &pane)["agent"]["state"], "working");
}

#[test]
fn a_backgrounded_conversation_marks_the_tab_it_came_from() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    // The tab's Claude parked job j1, which runs on with no pane of its own.
    session_file(
        &server.home,
        pid,
        json!({"sessionId": "tab", "parkedJobId": "j1"}),
    );
    session_file(
        &server.home,
        999_997,
        json!({"sessionId": "job", "jobId": "j1"}),
    );
    server.ok(&["new-tab", "-n", "other"]);
    client.wait_text("2:other");
    let got = run_hook(
        &server.home,
        &server.state,
        None,
        "Stop",
        &json!({"session_id": "job"}).to_string(),
    );
    assert_eq!(got, (0, String::new(), String::new()));
    assert_eq!(pane_of(&server.list(), &pane)["agent"]["state"], "done");
}

#[test]
fn a_second_claude_in_a_tab_gets_a_tab_of_its_own() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (first, first_pid) = first_pane(&server);
    let second = server.ok(&["split", "-t", &first]).trim().to_string();
    // Its shell at its prompt, so the shell is what is in front: while a
    // login profile runs a command (as a Linux computer's /etc/profile.d
    // may), that command is, and has no session file.
    client.wait_for("the second shell's prompt", |_| {
        server.ok(&["capture-pane", "-t", &second]).contains('$')
    });
    let second_pid = pane_of(&server.list(), &second)["pid"].as_u64().unwrap();
    session_file(&server.home, first_pid, json!({"sessionId": "a"}));
    session_file(&server.home, second_pid, json!({"sessionId": "b"}));
    hook(
        &server,
        &second,
        "SessionStart",
        json!({"source": "startup"}),
    );
    let list = server.list();
    let tabs = tabs(&list);
    assert_eq!(tabs.len(), 2, "{list}");
    assert_eq!(tabs[0]["panes"].as_array().unwrap().len(), 1);
    assert_eq!(tabs[0]["panes"][0]["id"], first.as_str());
    assert_eq!(tabs[1]["panes"][0]["id"], second.as_str());
    assert_eq!(tabs[1]["current"], true);
    // A session restarted by compaction is the same one going on: it stays.
    let third = server.ok(&["split", "-t", &first]).trim().to_string();
    let third_pid = pane_of(&server.list(), &third)["pid"].as_u64().unwrap();
    session_file(&server.home, third_pid, json!({"sessionId": "c"}));
    hook(
        &server,
        &third,
        "SessionStart",
        json!({"source": "compact"}),
    );
    assert_eq!(tab_count(&server), 2);
}

fn tab_count(server: &TestServer) -> usize {
    tabs(&server.list()).len()
}

#[test]
fn the_keys_go_to_what_needs_you_to_claude_and_pin() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "s"}));
    server.ok(&["new-tab", "-n", "other"]);
    client.wait_text("2:other");
    hook(
        &server,
        &pane,
        "PermissionRequest",
        json!({"tool_name": "Bash", "tool_input": {"command": "git push"}}),
    );
    assert_eq!(
        pane_of(&server.list(), &pane)["agent"]["wait"],
        "permission: Bash git push"
    );
    // M-a: to the waiting tab.
    client.type_text(&format!("{ALT}a"));
    wait_list(&server, "the waiting tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    // M-c: from a pane beside it, back into the pane running Claude.
    client.type_text(&format!("{ALT}L"));
    wait_list(&server, "a second pane focused", |l| {
        tabs(l)[0]["panes"].as_array().unwrap().len() == 2 && pane_of(l, &pane)["active"] == false
    });
    client.type_text(&format!("{ALT}c"));
    wait_list(&server, "Claude's pane focused", |l| {
        pane_of(l, &pane)["active"] == true
    });
    // M-f pins the tab, and again unpins it.
    client.type_text(&format!("{ALT}f"));
    wait_list(&server, "the tab pinned", |l| {
        tabs(l)[0]["options"]["@pinned"] == "1"
    });
    client.type_text(&format!("{ALT}f"));
    wait_list(&server, "the tab unpinned", |l| {
        tabs(l)[0]["options"].get("@pinned").is_none()
    });
}

#[test]
fn a_hook_outside_the_workspace_says_nothing_and_succeeds() {
    let dir = TempDir::new("sbt-hook");
    let home = dir.path().join("home");
    let state = dir.path().join("s");
    std::fs::create_dir_all(&home).unwrap();
    let silent = (0, String::new(), String::new());
    // No server at all, outside a pane and in one.
    assert_eq!(run_hook(&home, &state, None, "Stop", "{}"), silent);
    let missing = state.join("sock");
    assert_eq!(
        run_hook(&home, &state, Some((&missing, "%1")), "Stop", "{}"),
        silent
    );
    // JSON that can't be read, no event, and nothing on stdin.
    assert_eq!(
        run_hook(&home, &state, Some((&missing, "%1")), "Stop", "{not json"),
        silent
    );
    assert_eq!(run_hook(&home, &state, None, "", ""), silent);

    // While the Slack relay runs, its events file hears of the turn.
    let relay = home.join(".local/state/slack-relay");
    std::fs::create_dir_all(&relay).unwrap();
    std::fs::write(relay.join("events.jsonl"), "").unwrap();
    let input = json!({"session_id": "s", "transcript_path": "/t.jsonl", "cwd": "/w", "last_assistant_message": "Done."});
    assert_eq!(
        run_hook(&home, &state, None, "Stop", &input.to_string()),
        silent
    );
    assert_eq!(
        run_hook(&home, &state, None, "PreToolUse", &input.to_string()),
        silent
    );
    let events = std::fs::read_to_string(relay.join("events.jsonl")).unwrap();
    let lines: Vec<Value> = events
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 1, "{events}");
    assert_eq!(lines[0]["event"], "done");
    assert_eq!(lines[0]["last"], "Done.");
    assert_eq!(lines[0]["transcript_path"], "/t.jsonl");
    // (Hooks reach Claude outside the workspace only when Switchboard is set
    // up the old way, in ~/.claude; as a plugin, only its panes' run them.)

    // A server running, a pane it doesn't know.
    let server = TestServer::start();
    let got = run_hook(
        &server.home,
        &server.state,
        Some((&server.socket(), "%99")),
        "Stop",
        "{}",
    );
    assert_eq!(got, silent);
}

/// What `$CLAUDE_CODE_PLUGIN_DIRS` is in the first pane, as its shell
/// echoes it.
fn plugin_dirs_in_a_pane(server: &TestServer) -> String {
    let mut client = server.attach(120, 24);
    client.wait_text("$");
    let (pane, _) = first_pane(server);
    server.ok(&[
        "send-keys",
        "-t",
        &pane,
        "-l",
        "echo \"dirs=[$CLAUDE_CODE_PLUGIN_DIRS]\"",
    ]);
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    let found = |text: &str| {
        text.lines()
            .find(|l| l.starts_with("dirs=["))
            .map(|l| l.trim_end().to_string())
    };
    client.wait_for("the echo", |_| {
        found(&server.ok(&["capture-pane", "-t", &pane])).is_some()
    });
    found(&server.ok(&["capture-pane", "-t", &pane])).unwrap()
}

#[test]
fn a_pane_s_claude_loads_switchboard_s_plugin_while_the_setting_is_on() {
    // A server with a state directory of its own (a test's, or a worktree's
    // build run by hand) shares your home, so it leaves the plugin your
    // workspace's panes load as it is, though its panes name it.
    let own = TestServer::start();
    let plugin = own.home.join(".local/share/switchboard/plugin");
    assert_eq!(
        plugin_dirs_in_a_pane(&own),
        format!("dirs=[{}]", plugin.display())
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(!plugin.exists(), "not written: {}", own.log());

    // Yours writes it as it starts.
    let server = TestServer::start_in_home();
    let plugin = server.home.join(".local/share/switchboard/plugin");
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    while !plugin.join(".claude-plugin/plugin.json").is_file() {
        assert!(Instant::now() < deadline, "no plugin: {}", server.log());
        std::thread::sleep(Duration::from_millis(50));
    }
    let hooks = std::fs::read_to_string(plugin.join("hooks/hooks.json")).unwrap();
    assert!(hooks.contains("switchboard claude-context"), "{hooks}");
    assert!(plugin.join("skills/wt/SKILL.md").is_file());
    assert_eq!(
        plugin_dirs_in_a_pane(&server),
        format!("dirs=[{}]", plugin.display())
    );

    // Off: setup keeps Switchboard in ~/.claude, and the panes name no
    // plugin.
    let dir = TempDir::new("sbt-plugin-off");
    let file = dir.path().join("settings.json");
    common::write_settings_to(
        &file,
        serde_json::json!({"claude_plugin": false, "notifications": false}),
    );
    let off = TestServer::start_with_env(&[("SWITCHBOARD_SETTINGS", file.to_str().unwrap())]);
    assert_eq!(plugin_dirs_in_a_pane(&off), "dirs=[]");
}

/// Stand-ins for the desktop's clipboard tools, first on the PATH, so a
/// copy reaches a file in the test's home (`copied`) rather than the real
/// clipboard.
fn stand_in_clipboard(server: &TestServer) {
    stand_in::desktop(
        &server.home.join("bin"),
        stand_in::CLIPBOARD,
        &server.home.join("clipboard-log"),
        &server.home.join("copied"),
    );
}

#[test]
fn m_y_copies_the_latest_reply_of_the_tabs_claude() {
    let server = TestServer::start();
    settings(&server, json!({}));
    stand_in_clipboard(&server);
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    client.type_text(&format!("{ALT}y"));
    client.wait_text("No Claude in this tab");
    client.type_text("\x1b");
    client.wait_for("the box closed", |s| {
        !s.iter().any(|l| l.contains("No Claude"))
    });

    // The pane's shell stands in for Claude, its transcript in the
    // project's directory, with a prompt and no reply yet.
    let (pane, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "s-y", "cwd": "/w"}));
    let project = server.home.join(".claude/projects/-w");
    std::fs::create_dir_all(&project).unwrap();
    let transcript = project.join("s-y.jsonl");
    let prompt = json!({"type": "user", "message": {"content": "explain"}});
    std::fs::write(&transcript, format!("{prompt}\n")).unwrap();
    client.type_text(&format!("{ALT}y"));
    client.wait_text("Claude hasn't replied yet in this tab");
    client.type_text("\x1b");

    let said = [
        json!({"type": "assistant", "message": {"id": "m", "content": [{"type": "text", "text": "It works\nlike this."}]}}),
        json!({"type": "assistant", "message": {"id": "m", "content": [{"type": "text", "text": "And so."}]}}),
    ];
    std::fs::write(&transcript, format!("{prompt}\n{}\n{}\n", said[0], said[1])).unwrap();
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        json!({"session_id": "s-y"}),
    );
    client.type_text(&format!("{ALT}y"));
    client.wait_text("Copied Claude's latest reply (4 lines), written while it is");
    // Copied through the terminal (OSC 52), as every copy is, whichever of
    // the desktop's clipboard programs this computer has, if any.
    let osc = format!(
        "\x1b]52;c;{}\x07",
        switchboard::util::base64(b"It works\nlike this.\n\nAnd so.")
    );
    client.wait_for("the reply copied", |_| true);
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    while !String::from_utf8_lossy(&client.raw).contains(&osc) {
        assert!(Instant::now() < deadline, "not copied");
        client.wait_for("more drawn", |_| true);
        std::thread::sleep(Duration::from_millis(50));
    }
    client.type_text("\x1b");

    // Off, the key reaches the pane.
    // The sidebar going too shows the settings were read again.
    settings(&server, json!({"copy_reply": false, "sidebar": false}));
    wait_list(&server, "the settings read", |l| {
        l["sessions"][0]["sidebar"]["columns"] == 0
    });
    server.ok(&["send-keys", "-t", &pane, "-l", "cat -v"]);
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    client.type_text(&format!("{ALT}yafter\r"));
    client.wait_text("^[yafter");
}

#[test]
fn m_shift_a_walks_the_idle_tabs_wrapping_round() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    // The first two tabs' shells stand in for an idle Claude; the third
    // runs none, so it is no idle tab.
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("2:second");
    server.ok(&["new-tab", "-n", "plain"]);
    client.wait_text("3:plain");
    for (n, t) in tabs(&server.list()).iter().take(2).enumerate() {
        let pid = t["panes"][0]["pid"].as_u64().unwrap();
        session_file(&server.home, pid, json!({"sessionId": format!("s-{n}")}));
    }
    wait_list(&server, "both sessions read", |l| {
        tabs(l)
            .iter()
            .take(2)
            .all(|t| t["panes"][0]["claude"]["session"].is_string())
    });
    let at = |n: usize| move |l: &Value| tabs(l)[n]["current"] == true;
    client.type_text(&format!("{ALT}A"));
    wait_list(&server, "the first idle tab", at(0));
    client.type_text(&format!("{ALT}A"));
    wait_list(&server, "the next one", at(1));
    client.type_text(&format!("{ALT}A"));
    wait_list(&server, "round to the first again", at(0));

    // Off, the key reaches the pane.
    settings(&server, json!({"idle_walk": false, "sidebar": false}));
    wait_list(&server, "the settings read", |l| {
        l["sessions"][0]["sidebar"]["columns"] == 0
    });
    let pane = tabs(&server.list())[0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["send-keys", "-t", &pane, "-l", "cat -v"]);
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    client.type_text(&format!("{ALT}Aafter\r"));
    client.wait_text("^[Aafter");
}

/// `switchboard claude-context` as a SessionStart hook runs it in `pane`:
/// the context it adds.
fn context_in(server: &TestServer, pane: &str) -> String {
    let mut child = Command::new(common::BIN)
        .arg("claude-context")
        .env("HOME", &server.home)
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env("SWITCHBOARD", server.socket())
        .env("SWITCHBOARD_PANE", pane)
        .env("CLAUDE_CODE_ENTRYPOINT", "cli")
        .env_remove("NOTES_DIR")
        .env_remove("SWITCHBOARD_SETTINGS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"session_id":"s1","hook_event_name":"SessionStart"}"#)
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success() && out.stderr.is_empty());
    let answer: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    answer["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn a_session_starting_in_a_tab_with_a_note_is_pointed_at_it() {
    let server = TestServer::start();
    settings(&server, json!({"tab_note_context": true}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (pane, _) = first_pane(&server);
    let tab = tabs(&server.list())[0]["id"].as_str().unwrap().to_string();
    let notes = server.home.join("Notes");
    std::fs::create_dir_all(notes.join("Plans")).unwrap();
    std::fs::write(notes.join("Plans/Design.md"), "# Design\n").unwrap();
    std::fs::write(notes.join("Last.md"), "# Last\n").unwrap();
    let pointed = |note: &Path| format!("This tab's note is `{}`", notes.join(note).display());

    // No note: the part alone.
    assert!(!context_in(&server, &pane).contains("This tab's note"));

    // The note last shown beside the tab, while it has none of its own.
    server.ok(&["set", "-t", &tab, "@tab_note_last", "Last.md"]);
    assert!(context_in(&server, &pane).contains(&pointed(Path::new("Last.md"))));

    // Its own note first; one whose file is gone, or outside the notes
    // directory, isn't pointed at.
    server.ok(&["set", "-t", &tab, "@tab_note", "Plans/Design.md"]);
    let context = context_in(&server, &pane);
    assert!(
        context.contains(&pointed(Path::new("Plans/Design.md"))),
        "{context}"
    );
    assert!(context.contains("before your first reply"));
    server.ok(&["set", "-t", &tab, "@tab_note", "../Gone.md"]);
    assert!(context_in(&server, &pane).contains(&pointed(Path::new("Last.md"))));

    // The setting off, as it is by default: the part alone.
    settings(&server, json!({}));
    assert!(!context_in(&server, &pane).contains("This tab's note"));
}

/// A hook in `pane`, which must succeed, giving what it printed.
fn hook_says(server: &TestServer, pane: &str, event: &str, input: Value) -> String {
    let (code, out, err) = run_hook(
        &server.home,
        &server.state,
        Some((&server.socket(), pane)),
        event,
        &input.to_string(),
    );
    assert_eq!((code, err.as_str()), (0, ""), "hook {event}");
    out
}

#[test]
fn a_hands_free_tab_has_its_questions_answered_for_claude() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "s-1"}));
    let tab = tabs(&server.list())[0]["id"].as_str().unwrap().to_string();
    let ask = |header: &str| {
        json!({"session_id": "s-1", "tool_name": "AskUserQuestion", "tool_input": {"questions": [
            {"header": header, "question": "Which way?", "options": [{"label": "A (Recommended)"}, {"label": "B"}]}
        ]}})
    };
    let asking_stop =
        json!({"session_id": "s-1", "last_assistant_message": "Built A.\n\nShould I do B too?"});

    // Unmarked, nothing is answered and the question waits on you.
    assert_eq!(hook_says(&server, &pane, "PreToolUse", ask("Way")), "");
    assert_eq!(pane_of(&server.list(), &pane)["agent"]["state"], "waiting");
    assert_eq!(hook_says(&server, &pane, "Stop", asking_stop.clone()), "");

    server.ok(&["set", "-t", &tab, "@autopilot_handsfree", "1"]);
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        json!({"session_id": "s-1", "prompt": "go"}),
    );
    let denied: Value =
        serde_json::from_str(&hook_says(&server, &pane, "PreToolUse", ask("Way"))).unwrap();
    assert_eq!(denied["hookSpecificOutput"]["permissionDecision"], "deny");
    assert_eq!(
        pane_of(&server.list(), &pane)["agent"]["state"],
        "working",
        "a question answered for Claude waits on no one"
    );
    assert_eq!(
        hook_says(&server, &pane, "PreToolUse", ask("[always ask]")),
        "",
        "a question a skill marks always asked goes through"
    );
    assert_eq!(
        hook_says(&server, &pane, "PreToolUse", json!({"tool_name": "Bash"})),
        "",
        "other tools are left alone"
    );

    // A turn ending on a question carries on, three times an item.
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        json!({"session_id": "s-1", "prompt": "go"}),
    );
    assert_eq!(
        hook_says(
            &server,
            &pane,
            "Stop",
            json!({"session_id": "s-1", "last_assistant_message": "Waiting on the tests."})
        ),
        "",
        "a turn that asks nothing stops"
    );
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        json!({"session_id": "s-1", "prompt": "go"}),
    );
    for n in 1..=3 {
        let blocked: Value =
            serde_json::from_str(&hook_says(&server, &pane, "Stop", asking_stop.clone())).unwrap();
        assert_eq!(blocked["decision"], "block", "block {n}");
        assert_eq!(pane_of(&server.list(), &pane)["agent"]["state"], "working");
    }
    assert_eq!(
        server.ok(&["show", "-t", &tab, "@autopilot_nudged"]).trim(),
        "3"
    );
    assert_eq!(
        hook_says(&server, &pane, "Stop", asking_stop.clone()),
        "",
        "the fourth stands"
    );
    // Finished in the tab in front, so seen at once: no longer working.
    assert_ne!(pane_of(&server.list(), &pane)["agent"]["state"], "working");

    // Once landed, a stop stands whatever it asks.
    server.ok(&["set", "-t", &tab, "@autopilot_nudged", "0"]);
    server.ok(&["set", "-t", &tab, "@autopilot_autoland", "abc1234"]);
    assert_eq!(
        hook_says(&server, &pane, "Stop", asking_stop.clone()),
        "",
        "pushed"
    );
    server.ok(&["set", "-t", &tab, "-u", "@autopilot_autoland"]);
    server.ok(&["set", "-t", &tab, "@autopilot_pushed", "abc1234"]);
    assert_eq!(
        hook_says(&server, &pane, "PreToolUse", ask("Way")),
        "",
        "after the push, questions are asked as usual"
    );
    assert_eq!(
        hook_says(&server, &pane, "Stop", asking_stop.clone()),
        "",
        "pushed after asking"
    );
    server.ok(&["set", "-t", &tab, "-u", "@autopilot_pushed"]);
    server.ok(&["set", "-t", &tab, "@autopilot_left", "1"]);
    assert_eq!(
        hook_says(&server, &pane, "Stop", asking_stop),
        "",
        "back in the main clone"
    );
}

#[test]
fn a_tab_follows_the_directory_its_claude_works_in_and_never_a_removed_one() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (pane, _) = first_pane(&server);
    let cwd_of = |list: &Value| pane_of(list, &pane)["cwd"].as_str().map(String::from);

    // Claude's hooks say where its Bash tool has moved to, which its own
    // process (here the shell in front) never follows.
    let moved = server.home.join("moved");
    std::fs::create_dir_all(&moved).unwrap();
    let moved = moved.canonicalize().unwrap();
    hook(
        &server,
        &pane,
        "PostToolUse",
        json!({"session_id": "s-1", "cwd": moved, "tool_name": "Bash"}),
    );
    let said = moved.to_string_lossy().into_owned();
    wait_list(&server, "the reported directory", |l| {
        cwd_of(l).as_deref() == Some(&said)
    });

    // A directory removed under the pane is never shown: its nearest
    // ancestor that is still there is.
    let gone = moved.join("worktree");
    std::fs::create_dir_all(&gone).unwrap();
    hook(
        &server,
        &pane,
        "PostToolUse",
        json!({"session_id": "s-1", "cwd": gone, "tool_name": "Bash"}),
    );
    let gone_text = gone.to_string_lossy().into_owned();
    wait_list(&server, "the worktree", |l| {
        cwd_of(l).as_deref() == Some(&gone_text)
    });
    std::fs::remove_dir_all(&gone).unwrap();
    wait_list(&server, "the worktree's parent", |l| {
        cwd_of(l).as_deref() == Some(&said)
    });
}

#[test]
fn a_session_ending_hands_the_tab_back_to_its_program_s_directory() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (pane, _) = first_pane(&server);
    let cwd_of = |list: &Value| pane_of(list, &pane)["cwd"].as_str().map(String::from);
    let before = cwd_of(&server.list());

    let moved = server.home.join("moved");
    std::fs::create_dir_all(&moved).unwrap();
    let moved = moved.canonicalize().unwrap();
    hook(
        &server,
        &pane,
        "PostToolUse",
        json!({"session_id": "s-1", "cwd": moved, "tool_name": "Bash"}),
    );
    let said = moved.to_string_lossy().into_owned();
    wait_list(&server, "the reported directory", |l| {
        cwd_of(l).as_deref() == Some(&said)
    });
    hook(
        &server,
        &pane,
        "SessionEnd",
        json!({"session_id": "s-1", "cwd": moved, "reason": "exit"}),
    );
    wait_list(&server, "the shell's own directory", |l| {
        cwd_of(l) == before
    });
}
