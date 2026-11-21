//! The phone app's commands (src/phone/serve.rs) against a server of the
//! test's own: `prompt` reading a prompt off a Claude pane's screen,
//! `answer` pressing its option or typing a message, `watch` streaming the
//! Claude tabs as they change, with each one's repository and branch, and
//! each refusing a pane not running Claude; and the app's drawer search,
//! run in Node.
//! The shell stands in for Claude (a session file for its pid, as Claude
//! Code writes one), and prints a prompt as Claude Code draws one; `cat`
//! then shows what was pressed.

use crate::common;

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use common::TestServer;
use serde_json::{Value, json};
use switchboard::mux::proto::{self, ClientMsg, ServerMsg};

/// The first tab's first pane, its shell standing in for Claude once the
/// sidebar has read it as Claude's.
fn claude_pane(server: &TestServer) -> String {
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0].clone();
    let pid = pane["pid"].as_u64().unwrap();
    let dir = server.home.join(".claude/sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let file = json!({"pid": pid, "entrypoint": "cli", "sessionId": "s-phone"});
    std::fs::write(dir.join(format!("{pid}.json")), file.to_string()).unwrap();
    wait_until(server, "the pane read as Claude's", || {
        server.list()["sessions"][0]["tabs"][0]["panes"][0]["runs_claude"] == true
    });
    pane["id"].as_str().unwrap().to_string()
}

fn wait_until(server: &TestServer, what: &str, done: impl Fn() -> bool) {
    let deadline = common::deadline(Duration::from_secs(10));
    while !done() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}\nserver log: {}",
            server.log()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn screen(server: &TestServer, pane: &str) -> String {
    server.ok(&["capture-pane", "-t", pane])
}

/// Claude Code's permission prompt, printed, then `cat` echoing what is
/// pressed. Its ─ and ❯ go as octal escapes, the only ones POSIX printf
/// reads: Linux's sh is dash, whose printf has no `\x`.
fn show_prompt(server: &TestServer, pane: &str) {
    server.ok(&[
        "send-keys",
        "-t",
        pane,
        "-l",
        "clear; printf '\\342\\224\\200\\342\\224\\200\\342\\224\\200\\nDo you want to proceed?\\n\\342\\235\\257 1. Yes\\n  2. No\\n'; cat",
    ]);
    server.ok(&["send-keys", "-t", pane, "Enter"]);
    wait_until(server, "the prompt shown", || {
        screen(server, pane).lines().any(|l| l.trim() == "2. No")
    });
}

#[test]
fn prompt_and_answer_read_and_press_a_claude_panes_prompt() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let pane = claude_pane(&server);

    let none: Value = serde_json::from_str(&server.ok(&["prompt", "-t", &pane])).unwrap();
    assert_eq!(none, Value::Null, "no prompt at a shell's prompt");
    let (code, _, err) = server.run(&["answer", "-t", &pane, "1"]);
    assert_eq!(code, 1);
    assert!(err.contains("isn't showing a prompt"), "{err}");

    show_prompt(&server, &pane);
    let found: Value = serde_json::from_str(&server.ok(&["prompt", "-t", &pane])).unwrap();
    assert_eq!(found["question"], "Do you want to proceed?");
    assert_eq!(
        found["options"][1],
        json!({"n": 2, "label": "No", "checked": null})
    );

    let (code, _, err) = server.run(&["answer", "-t", &pane, "maybe"]);
    assert_eq!(code, 1);
    assert!(err.contains("isn't one of its options"), "{err}");
    server.ok(&["set", "-t", &pane, "@agent_state", "waiting"]);
    let (code, _, err) = server.run(&["answer", "-t", &pane, "-m", "hello"]);
    assert_eq!(code, 1, "a message waits for the prompt to be answered");
    assert!(err.contains("showing a prompt"), "{err}");

    let id = found["id"].as_str().unwrap().to_string();
    let (code, _, err) = server.run(&["answer", "-t", &pane, "-h", "not-this-one", "2"]);
    assert_eq!(code, 1, "an answer meant for another prompt");
    assert!(err.contains("prompt changed"), "{err}");

    let said = server.ok(&["answer", "-t", &pane, "-h", &id, "-s", "phone", "2"]);
    assert_eq!(said.trim(), "Chose 2. No");
    wait_until(&server, "2 pressed", || {
        screen(&server, &pane).lines().any(|l| l.trim() == "2")
    });
    assert!(
        server.log().contains("from your phone: tab 1 ("),
        "logged: {}",
        server.log()
    );
}

#[test]
fn a_message_reaches_a_claude_showing_no_prompt() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let pane = claude_pane(&server);
    server.ok(&["send-keys", "-t", &pane, "-l", "clear; cat"]);
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    std::thread::sleep(Duration::from_millis(200) * common::patience());
    server.ok(&["answer", "-t", &pane, "-m", "keep going"]);
    wait_until(&server, "the message typed", || {
        screen(&server, &pane).contains("keep going")
    });
}

#[test]
fn nothing_reaches_a_pane_not_running_claude() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    for args in [
        vec!["prompt", "-t", &pane],
        vec!["answer", "-t", &pane, "1"],
        vec!["answer", "-t", &pane, "-m", "hi"],
    ] {
        let (code, _, err) = server.run(&args);
        assert_eq!(code, 1, "{args:?}");
        assert!(err.contains("isn't running Claude"), "{args:?}: {err}");
    }
}

#[test]
fn answer_typed_in_a_pane_is_refused() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let pane = claude_pane(&server);
    show_prompt(&server, &pane);
    let (code, _, err) = server.typed_in(&pane, &["answer", "-t", &pane, "1"]);
    assert_eq!(code, 1);
    assert!(err.contains("not typed in a pane"), "{err}");
}

/// The next line `watch` sends that `pred` takes.
fn next_line(stream: &mut UnixStream, pred: impl Fn(&Value) -> bool) -> Value {
    let deadline = common::deadline(Duration::from_secs(10));
    loop {
        assert!(Instant::now() < deadline, "watch never sent it");
        match proto::read_server(stream) {
            Ok(Some(ServerMsg::Output(bytes))) => {
                for line in String::from_utf8_lossy(&bytes).lines() {
                    let v: Value = serde_json::from_str(line).unwrap();
                    if pred(&v) {
                        return v;
                    }
                }
            }
            Ok(other) => panic!("watch sent {other:?}"),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => panic!("watch: {e}"),
        }
    }
}

#[test]
fn watch_streams_the_claude_tabs_as_they_change() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-d", "-n", "plain"]);
    let pane = claude_pane(&server);

    let mut stream = UnixStream::connect(server.socket()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let msg = ClientMsg::Command {
        args: vec!["watch".into()],
        cwd: server.home.clone(),
        pane: None,
        stdin: None,
        by_hand: false,
    };
    stream.write_all(&proto::encode_client(&msg)).unwrap();
    let first = next_line(&mut stream, |v| v["tabs"].as_array().is_some());
    let tabs = first["tabs"].as_array().unwrap();
    assert_eq!(tabs.len(), 1, "the Claude tab alone: {first}");
    assert_eq!(tabs[0]["pane"], pane);
    assert_eq!(tabs[0]["needs"], false);

    show_prompt(&server, &pane);
    server.ok(&["set", "-t", &pane, "@agent_state", "waiting"]);
    let waiting = next_line(&mut stream, |v| v["tabs"][0]["state"] == "waiting");
    assert_eq!(waiting["tabs"][0]["needs"], true);
    assert_eq!(waiting["tabs"][0]["prompt"]["options"][0]["label"], "Yes");
}

#[test]
fn the_tabs_the_phone_reads_carry_their_repository_and_branch() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    // The first tab's directory made a repository on a branch, by git's
    // own files, which the tab's name is read from.
    let git = server.home.join(".git");
    std::fs::create_dir_all(git.join("refs/heads")).unwrap();
    std::fs::write(git.join("HEAD"), "ref: refs/heads/phone-search\n").unwrap();
    std::fs::write(
        git.join("refs/heads/phone-search"),
        "0123456789abcdef0123456789abcdef01234567\n",
    )
    .unwrap();
    let pane = claude_pane(&server);

    let mut stream = UnixStream::connect(server.socket()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let msg = ClientMsg::Command {
        args: vec!["watch".into()],
        cwd: server.home.clone(),
        pane: None,
        stdin: None,
        by_hand: false,
    };
    stream.write_all(&proto::encode_client(&msg)).unwrap();
    let seen = next_line(&mut stream, |v| v["tabs"][0]["branch"] == "phone-search");
    let tab = &seen["tabs"][0];
    assert_eq!(tab["pane"], pane);
    let home_name = server.home.canonicalize().unwrap();
    let home_name = home_name.file_name().unwrap().to_string_lossy();
    assert_eq!(tab["repo"], *home_name, "{seen}");
}

/// The drawer's search (assets/phone/app.js's `groups`), run in Node over
/// tabs as the phone reads them; skipped without Node.
#[test]
fn the_drawers_search_keeps_the_tabs_matching_in_their_groups() {
    let app = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/phone/app.js");
    // The page's own globals stood in for, its start left waiting on a
    // fetch that never answers, so only its functions run.
    let script = r#"
const vm = require("vm");
const fs = require("fs");
const ctx = vm.createContext({
  document: { getElementById: () => null, addEventListener: () => {} },
  fetch: () => new Promise(() => {}),
  console,
});
vm.runInContext(fs.readFileSync(process.argv[1], "utf8"), ctx);
const run = (q) => vm.runInContext(`
  state.tabs = [
    { index: 2, name: "stable note IDs", repo: "switchboard", branch: "stable-note-ids", state: "waiting", needs: true, wait: "Bash: git push" },
    { index: 3, name: "M-d uncommitted", repo: "switchboard", branch: "diff-uncommitted", state: "working", wait: "Edit diff.rs" },
    { index: 8, name: "handoff", repo: "notes-sync", branch: "main", state: "idle" },
  ];
  JSON.stringify(groups(wordsOf(${JSON.stringify(q)})).map(([name, , tabs]) => [name, tabs.map((t) => t.index)]));
`, ctx);
for (const q of ["", "HANDOFF", "3", "diff-UNCOMMITTED", "note sync", "deploy"]) console.log(q + "=" + run(q));
"#;
    let out = match std::process::Command::new("node")
        .arg("-e")
        .arg(script)
        .arg(&app)
        .output()
    {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return eprintln!("skipped: no node");
        }
        result => result.unwrap(),
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            r#"=[["Needs you",[2]],["Working",[3]],["Idle",[8]]]"#,
            // By name, case aside: the other groups, left with none, dropped.
            r#"HANDOFF=[["Idle",[8]]]"#,
            // By number.
            r#"3=[["Working",[3]]]"#,
            // By branch.
            r#"diff-UNCOMMITTED=[["Working",[3]]]"#,
            // Every word typed: "note" alone would keep tab 2 too.
            r#"note sync=[["Idle",[8]]]"#,
            "deploy=[]",
        ],
        "{stdout}"
    );
}
