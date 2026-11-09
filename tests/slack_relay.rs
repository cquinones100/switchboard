//! The Slack relay against a server of the test's own: the tabs as the
//! relay reads them from `list` (each pane's Claude session and transcript,
//! as the server found it in front on the pane), typing and pasting into a
//! pane and reading its screen back, its options, renaming and closing a
//! tab, and the command's own: `start` opening the relay's tab once, and
//! `stop`, `status` and `notify` with no relay running. Slack is never
//! reached: the relay the tab starts finds no tokens in the test's home and
//! stops at once.
//!
//! As in tests/agent.rs, a pane counts as running Claude when a process in
//! front on its terminal has a session file, so the tests write one for the
//! pane's own process, or when its program is named as Claude Code's is,
//! which one test runs a stand-in for.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use common::TestServer;
use common::stand_in::Claude;
use serde_json::{Value, json};
use switchboard::relay::mux::{Mux, Server};

fn settings(server: &TestServer) {
    common::write_settings(
        &server.home,
        json!({ "notifications": false, "tips": false }),
    );
}

/// Waits until `pred` holds of the relay's view of the tabs.
fn wait<T>(what: &str, mut pred: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(found) = pred() {
            return found;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `switchboard slack-relay <args>` against the test's server and home.
fn relay(server: &TestServer, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(common::BIN)
        .arg("slack-relay")
        .args(args)
        .env("HOME", &server.home)
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env("SWITCHBOARD", server.socket())
        .env_remove("SWITCHBOARD_PANE")
        .env_remove("TMUX")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn the_relay_reads_and_types_into_tabs_through_the_server() {
    let server = TestServer::start();
    settings(&server);
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let opened = server.ok(&["new-tab", "-d", "-n", "work", "--", "cat"]);
    let mut ids = opened.split_whitespace();
    let (tab, pane) = (
        ids.next().unwrap().to_string(),
        ids.next().unwrap().to_string(),
    );
    let mux = Server {
        socket: server.socket(),
    };
    let pid = wait("the pane's process", || {
        mux.windows()?
            .into_iter()
            .find(|w| w.id == tab)?
            .panes
            .first()
            .map(|p| p.pid.clone())
            .filter(|p| !p.is_empty())
    });
    // Claude Code's session file for the pane's process, and its transcript.
    let sessions = server.home.join(".claude/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join(format!("{pid}.json")),
        json!({ "pid": pid.parse::<u64>().unwrap(), "sessionId": "relay-s1", "entrypoint": "cli", "cwd": "/w" }).to_string(),
    )
    .unwrap();
    let projects = server.home.join(".claude/projects/-w");
    std::fs::create_dir_all(&projects).unwrap();
    let transcript: PathBuf = projects.join("relay-s1.jsonl");
    std::fs::write(
        &transcript,
        json!({ "type": "user", "message": { "content": "hi" } }).to_string(),
    )
    .unwrap();

    let window = wait("the tab's Claude and transcript", || {
        mux.windows()?.into_iter().find(|w| {
            w.id == tab
                && w.panes
                    .first()
                    .is_some_and(|p| p.claude && p.transcript.is_some())
        })
    });
    assert_eq!(window.panes[0].id, pane);
    assert_eq!(
        window.panes[0]
            .transcript
            .as_deref()
            .map(Path::canonicalize)
            .and_then(Result::ok),
        Some(transcript.canonicalize().unwrap())
    );
    assert_eq!(window.panes[0].state, "idle");
    assert!(!window.relay);

    // A message goes in as one paste, its line break kept, then Enter.
    mux.paste(&pane, "hello\nworld").unwrap();
    mux.send_keys(&pane, &["Enter"]).unwrap();
    // Each line of the paste on a line of its own, waited for whole: cat
    // prints the first line while the terminal still echoes the second, so
    // "worldhello" is on the way there.
    wait("each line of the paste on a line of its own", || {
        mux.capture(&pane, 60)
            .filter(|s| s.lines().any(|l| l == "hello") && s.lines().any(|l| l == "world"))
    });
    mux.type_text(&pane, "2").unwrap();
    wait("a number typed", || {
        mux.capture(&pane, 60)
            .filter(|s| s.lines().any(|l| l.ends_with('2')))
    });

    // Its Claude pane recorded on the tab, a name given and taken back.
    mux.set_option(&tab, "@claude_pane", Some(&pane)).unwrap();
    mux.rename_tab(&tab, "Relay test").unwrap();
    let named = mux
        .windows()
        .unwrap()
        .into_iter()
        .find(|w| w.id == tab)
        .unwrap();
    assert_eq!(named.recorded.as_deref(), Some(pane.as_str()));
    assert_eq!(named.own_name, "Relay test");
    assert!(
        named.name.contains("Relay test"),
        "the label follows: {}",
        named.name
    );
    mux.rename_tab(&tab, "").unwrap();
    assert_eq!(
        mux.windows()
            .unwrap()
            .into_iter()
            .find(|w| w.id == tab)
            .unwrap()
            .own_name,
        ""
    );

    mux.kill_tab(&tab).unwrap();
    assert!(!mux.windows().unwrap().iter().any(|w| w.id == tab));
}

#[test]
fn a_claude_with_no_session_file_yet_still_counts() {
    // Claude Code asks whether to trust a folder before it writes its
    // session file; a program named as its executable is counts as Claude
    // already, so the relay can alert on that question and answer it.
    let server = TestServer::start();
    settings(&server);
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    // Claude Code at its first question, waiting for an answer.
    let claude = Claude::new()
        .answer("echo 'Do you trust the files in this folder?'\nwhile :; do sleep 1; done")
        .install(&server.home.join("bin"));
    let opened = server.ok(&[
        "new-tab",
        "-d",
        "-n",
        "trust",
        "--",
        claude.to_str().unwrap(),
    ]);
    let tab = opened.split_whitespace().next().unwrap().to_string();
    let other = server.ok(&["new-tab", "-d", "-n", "plain", "--", "cat"]);
    let plain = other.split_whitespace().next().unwrap().to_string();
    let mux = Server {
        socket: server.socket(),
    };
    let window = wait("the tab counted as Claude", || {
        mux.windows()?
            .into_iter()
            .find(|w| w.id == tab && w.panes.first().is_some_and(|p| p.claude))
    });
    assert!(window.panes[0].transcript.is_none(), "no session yet");
    let cat = mux
        .windows()
        .unwrap()
        .into_iter()
        .find(|w| w.id == plain)
        .unwrap();
    assert!(!cat.panes[0].claude);
    assert_eq!(cat.panes[0].command, "cat");
    mux.kill_tab(&tab).unwrap();
    mux.kill_tab(&plain).unwrap();
}

#[test]
fn start_opens_the_relays_tab_once_and_the_rest_say_it_isnt_running() {
    let server = TestServer::start();
    settings(&server);
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (code, _, err) = relay(&server, &["stop"]);
    assert_eq!(
        (code, err.as_str()),
        (1, "Error: the relay is not running\n")
    );
    let (code, out, _) = relay(&server, &["status"]);
    assert_eq!((code, out.as_str()), (0, "The relay is not running.\n"));
    // How long since a key was pressed in the workspace, which the relay
    // goes by where the desktop can't say how long you've been idle.
    assert!(server.list()["idle_ms"].is_u64());
    let (code, _, err) = relay(&server, &["notify", "--from", "oncall", "hello"]);
    assert_eq!(code, 1);
    assert!(err.contains("the relay isn't running"), "{err}");
    // `--from` with no name after it falls back to `notice` rather than
    // being refused as an empty name.
    let (code, _, err) = relay(&server, &["notify", "hello", "--from"]);
    assert_eq!(code, 1);
    assert!(err.contains("the relay isn't running"), "{err}");
    // An unknown command is said, and written in the relay's log.
    let (code, _, err) = relay(&server, &["frobnicate"]);
    assert_eq!(code, 1);
    assert!(err.contains("unknown command \"frobnicate\""), "{err}");
    let log = std::fs::read_to_string(server.home.join(".local/state/slack-relay/relay.log"))
        .unwrap_or_default();
    assert!(log.contains("unknown command \"frobnicate\""), "{log}");

    let marked = |list: &Value| {
        list["sessions"][0]["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["options"]["@slack_relay"] == "1")
            .count()
    };
    assert_eq!(relay(&server, &["start"]).0, 0, "{}", server.log());
    assert_eq!(marked(&server.list()), 1);
    // Asked again, it goes to that tab rather than opening another.
    assert_eq!(relay(&server, &["start"]).0, 0);
    assert_eq!(marked(&server.list()), 1);
    let tab = server.list()["sessions"][0]["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["options"]["@slack_relay"] == "1")
        .map(|t| t["id"].as_str().unwrap().to_string())
        .unwrap();
    server.ok(&["kill-tab", "-t", &tab]);
}

/// The tabs marked as the relay's, by id, with their first pane's process.
fn relay_tabs(server: &TestServer) -> Vec<(String, String)> {
    server.list()["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["tabs"].as_array().unwrap().clone())
        .filter(|t| t["options"]["@slack_relay"] == "1")
        .map(|t| {
            (
                t["id"].as_str().unwrap().to_string(),
                t["panes"][0]["pid"].to_string(),
            )
        })
        .collect()
}

#[test]
fn start_from_outside_the_workspace_opens_the_relays_tab_in_the_running_server() {
    // No SWITCHBOARD in the environment, as from a terminal of its own or
    // ssh: the server is found by the state directory, the test's own.
    let server = TestServer::start();
    settings(&server);
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let out = Command::new(common::BIN)
        .args(["slack-relay", "start"])
        .env("HOME", &server.home)
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env_remove("SWITCHBOARD")
        .env_remove("SWITCHBOARD_PANE")
        .env_remove("TMUX")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", server.log());
    let tabs = relay_tabs(&server);
    assert_eq!(tabs.len(), 1, "the relay's tab");
    let list = server.list();
    let tab = list["sessions"][0]["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == tabs[0].0.as_str())
        .unwrap();
    assert!(
        tab["label"]
            .as_str()
            .unwrap_or_default()
            .contains("Slack relay")
            || tab["name"] == "Slack relay",
        "{tab}"
    );
    server.ok(&["kill-tab", "-t", &tabs[0].0]);
}

#[test]
fn start_runs_the_relay_again_in_its_tab_when_the_one_there_has_stopped() {
    let server = TestServer::start();
    settings(&server);
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    // The relay's tab, left at a program (cat stands in) by a relay that
    // stopped.
    let opened = server.ok(&["new-tab", "-d", "-n", "relay", "--", "cat"]);
    let tab = opened.split_whitespace().next().unwrap().to_string();
    server.ok(&["set", "-t", &tab, "@slack_relay", "1"]);
    let pid = || {
        relay_tabs(&server)
            .into_iter()
            .find(|(id, _)| *id == tab)
            .map(|(_, pid)| pid)
            .unwrap()
    };
    let before = pid();
    let lock = server.home.join(".local/state/slack-relay/relay.lock");
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    // A live relay holds the lock: its tab is only gone to.
    std::fs::write(&lock, format!("{} slack-relay\n", std::process::id())).unwrap();
    assert_eq!(relay(&server, &["start"]).0, 0);
    assert_eq!(pid(), before);
    // A stale lock: the relay is started again in the same tab.
    std::fs::write(&lock, "999999 slack-relay\n").unwrap();
    assert_eq!(relay(&server, &["start"]).0, 0, "{}", server.log());
    let deadline = Instant::now() + Duration::from_secs(10);
    while pid() == before {
        assert!(
            Instant::now() < deadline,
            "the tab's program wasn't started again"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(relay_tabs(&server).len(), 1, "no second relay tab");
    let _ = std::fs::remove_file(&lock);
    server.ok(&["kill-tab", "-t", &tab]);
}

#[test]
fn start_without_a_server_says_so() {
    let dir = common::TempDir::new("sbr");
    let out = Command::new(common::BIN)
        .args(["slack-relay", "start"])
        .env("HOME", dir.path())
        .env("SWITCHBOARD_STATE_DIR", dir.path().join("s"))
        .env_remove("SWITCHBOARD")
        .env_remove("SWITCHBOARD_PANE")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no Switchboard server is running"));
}
