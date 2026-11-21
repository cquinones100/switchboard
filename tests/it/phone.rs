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
        json!({"n": 2, "label": "No", "checked": null, "detail": ""})
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
fn the_prompt_the_phone_reads_has_each_choices_explainer() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let pane = claude_pane(&server);
    server.ok(&[
        "send-keys",
        "-t",
        &pane,
        "-l",
        "clear; printf 'Which fixture?\\n\\342\\235\\257 1. Small\\n     The fixture the PR adds\\n  2. Big\\n     The stress set\\n\\nEnter to select\\n'; cat",
    ]);
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    wait_until(&server, "the question shown", || {
        screen(&server, &pane)
            .lines()
            .any(|l| l.trim() == "The stress set")
    });
    let found: Value = serde_json::from_str(&server.ok(&["prompt", "-t", &pane])).unwrap();
    assert_eq!(found["options"][0]["detail"], "The fixture the PR adds");
    assert_eq!(found["options"][1]["detail"], "The stress set");
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

/// Runs `script` in Node with assets/phone/app.js's path as its argument,
/// returning what it printed, or None without Node.
fn run_app_js(script: &str) -> Option<String> {
    let app = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/phone/app.js");
    let out = match std::process::Command::new("node")
        .arg("-e")
        .arg(script)
        .arg(&app)
        .output()
    {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipped: no node");
            return None;
        }
        result => result.unwrap(),
    };
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(stdout)
}

/// An open tab's channel (assets/phone/app.js's `channel`), run in Node over
/// a page of its own: an update from the stream, and new turns, leave the
/// composer's field in place, focused and with what was typed, and a message
/// opened with Show all open; skipped without Node.
#[test]
fn an_update_keeps_the_composer_and_the_messages_opened() {
    let script = r##"
const vm = require("vm");
const fs = require("fs");
class Node {
  constructor(tag) {
    this.tagName = tag; this.children = []; this.parentNode = null; this.className = "";
    this.listeners = {}; this.style = {}; this.value = ""; this.text = "";
    this.scrollTop = 0; this.scrollHeight = 1000; this.clientHeight = 100;
    const node = this;
    this.classList = {
      has: (c) => node.className.split(" ").includes(c),
      contains: (c) => node.classList.has(c),
      add: (c) => { if (!node.classList.has(c)) node.className = (node.className + " " + c).trim(); },
      remove: (c) => { node.className = node.className.split(" ").filter((x) => x && x !== c).join(" "); },
      toggle: (c, on) => (on ? node.classList.add(c) : node.classList.remove(c)),
    };
  }
  get textContent() { return this.text + this.children.map((c) => c.textContent).join(""); }
  setAttribute(k, v) { this[k] = v; }
  addEventListener(k, f) { (this.listeners[k] = this.listeners[k] || []).push(f); }
  dispatchEvent(e) { for (const f of this.listeners[e.type] || []) f({ ...e, target: this }); }
  click() { this.dispatchEvent(new Event("click")); }
  focus() { document.activeElement = this; }
  append(...kids) { for (const k of kids) { if (k.parentNode) k.remove(); k.parentNode = this; this.children.push(k); } }
  remove() { const p = this.parentNode; if (p) { p.children = p.children.filter((c) => c !== this); this.parentNode = null; } }
  replaceChildren(...kids) { for (const c of this.children) c.parentNode = null; this.children = []; this.append(...kids); }
  replaceWith(n) { const p = this.parentNode; const i = p.children.indexOf(this); if (n.parentNode) n.remove(); p.children[i] = n; n.parentNode = p; this.parentNode = null; }
  contains(n) { for (; n; n = n.parentNode) if (n === this) return true; return false; }
  all() { return this.children.flatMap((c) => [c, ...c.all()]); }
  // Selectors of "tag.class" parts, each part a descendant of the one before.
  querySelectorAll(sel) {
    let found = [this];
    for (const part of sel.split(" ")) {
      const [tag, ...classes] = part.split(".");
      found = [...new Set(found.flatMap((n) => n.all()))].filter(
        (n) => (!tag || n.tagName === tag) && classes.every((c) => n.classList.has(c)),
      );
    }
    return found;
  }
  querySelector(sel) { return this.querySelectorAll(sel)[0] || null; }
}
class Event { constructor(type) { this.type = type; } }
const app = new Node("div");
const document = {
  activeElement: null,
  getElementById: () => app,
  addEventListener: () => {},
  createElement: (tag) => new Node(tag),
  createTextNode: (text) => Object.assign(new Node("#text"), { text }),
};
let source = null;
const ctx = vm.createContext({
  document, Node, Event, console,
  EventSource: class { constructor() { source = this; } },
  history: { replaceState: () => {} },
  window: { addEventListener: () => {} },
  location: { hash: "" },
  fetch: () => new Promise(() => {}),
});
vm.runInContext(fs.readFileSync(process.argv[1], "utf8"), ctx);
const tab = (state, wait) => ({ pane: "%5", index: 2, name: "phone", state, wait, turn: { prompt: "go" } });
const long = { who: "claude", text: "line\n".repeat(20), at: 1 };
vm.runInContext(`
  state.me = { paired: true, computer: "mac" };
  listen();
  state.open = "5";
`, ctx);
const update = (t) => source.onmessage({ data: JSON.stringify({ live: true, tabs: [t] }) });
update(tab("working", "Edit app.js"));
vm.runInContext("state.messages = " + JSON.stringify([long, { who: "you", text: "hi", at: 2 }]) + "; render(true);", ctx);
const field = app.querySelector(".composer textarea");
field.value = "half a thought";
field.focus();
app.querySelector("button.more").click();
update(tab("working", "Bash: cargo test"));
console.log("doing=" + app.querySelector(".doing").textContent);
console.log("same field=" + (app.querySelector(".composer textarea") === field));
console.log("typed=" + field.value);
console.log("focused=" + (document.activeElement === field && app.contains(field)));
console.log("opened=" + !app.querySelector(".body.folded"));
// New turns arriving, as loadTurns draws them.
vm.runInContext("state.messages = " + JSON.stringify([long, { who: "you", text: "hi", at: 2 }, { who: "claude", text: "done", at: 3 }]) + "; render(true);", ctx);
console.log("messages=" + app.querySelectorAll(".msg").length);
console.log("after turns=" + (app.querySelector(".composer textarea") === field) + " " + !app.querySelector(".body.folded"));
console.log("at end=" + (app.querySelector(".channel").scrollTop === 1000));
update(tab("waiting", ""));
console.log("chip=" + app.querySelector(".head .chip").textContent);
"##;
    let Some(stdout) = run_app_js(script) else {
        return;
    };
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "doing=⠹ Bash: cargo test",
            "same field=true",
            "typed=half a thought",
            "focused=true",
            "opened=true",
            "messages=3",
            "after turns=true true",
            "at end=true",
            "chip=waiting",
        ],
        "{stdout}"
    );
}

/// The drawer's search (assets/phone/app.js's `groups`), run in Node over
/// tabs as the phone reads them; skipped without Node.
#[test]
fn the_drawers_search_keeps_the_tabs_matching_in_their_groups() {
    // The page's own globals stood in for, its start left waiting on a
    // fetch that never answers, so only its functions run.
    let script = r#"
const vm = require("vm");
const fs = require("fs");
const ctx = vm.createContext({
  document: { getElementById: () => null, addEventListener: () => {} },
  window: { addEventListener: () => {} },
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
    let Some(stdout) = run_app_js(script) else {
        return;
    };
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
