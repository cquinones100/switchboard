//! The phone app's commands (src/phone/serve.rs) against a server of the
//! test's own: `prompt` reading a prompt off a Claude pane's screen,
//! `answer` pressing its option or typing a message, `watch` streaming the
//! Claude tabs as they change, and each refusing a pane not running Claude.
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

/// M-p's "Send a test notification to my phone" (`phone test`): a push to
/// each subscription of a device phone.json lists, through a stand-in push
/// service, what each answered said and kept in phone.log, and a
/// subscription the service says is gone forgotten. The push key is made
/// in the stand-in Keychain on a Mac, else in secrets.json.
#[test]
fn the_test_notification_reaches_each_phone_and_says_what_came_back() {
    use common::http::{Fake, Reply, Service};
    let server = TestServer::start();
    if switchboard::platform::NAME == "macOS" {
        common::stand_in::security(&server.home.join("bin"), &server.home.join("keychain"));
    }
    let config = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("phone.json"),
        json!({"owner": "me@example.com", "devices": ["phone", "tablet"]}).to_string(),
    )
    .unwrap();

    let (code, _, err) = server.run_binary(&["phone", "test"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("no phone has turned on notifications"),
        "{err}"
    );

    let push = Fake::start(Service::Web, |req| match req.path() {
        "/phone" => Some(Reply::bytes(201, "text/plain", "")),
        "/tablet" => Some(Reply::bytes(410, "text/plain", "")),
        _ => None,
    });
    // RFC 8291's example browser key and secret.
    let sub = |device: &str| {
        json!({
            "device": device,
            "endpoint": format!("{}/{device}", push.url),
            "p256dh": "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
            "auth": "BTBZMqHH6r4Tts7J_aSIgg",
        })
    };
    let dir = server.state.join("phone");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("push.json"),
        json!([sub("phone"), sub("tablet"), sub("unlisted")]).to_string(),
    )
    .unwrap();

    let (code, out, err) = server.run_binary(&["phone", "test"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("phone: sent (201)"), "{out}");
    assert!(out.contains("tablet: gone (410)"), "{out}");
    let paths: Vec<String> = push.seen().iter().map(|r| r.path().to_string()).collect();
    assert_eq!(
        paths,
        ["/phone", "/tablet"],
        "a device phone.json doesn't list gets nothing"
    );
    let log = std::fs::read_to_string(dir.join("phone.log")).unwrap();
    assert!(log.contains("push to phone: sent (201)"), "{log}");
    assert!(log.contains("push to tablet: gone (410)"), "{log}");
    let kept: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("push.json")).unwrap()).unwrap();
    let devices: Vec<&str> = kept
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["device"].as_str().unwrap())
        .collect();
    assert_eq!(devices, ["phone", "unlisted"]);
}
