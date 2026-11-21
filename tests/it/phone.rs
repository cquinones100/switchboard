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
fn what_the_phone_does_is_logged_and_never_a_notification() {
    let server = TestServer::start();
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    let pane = claude_pane(&server);

    show_prompt(&server, &pane);
    server.ok(&["answer", "-t", &pane, "-s", "phone", "1"]);
    server.ok(&["seen", "-t", &pane, "-s", "phone"]);
    let log = server.log();
    assert!(log.contains("from your phone: tab 1 ("), "logged: {log}");
    assert!(log.contains("): Chose 1. Yes"), "logged: {log}");
    assert!(log.contains("): Marked seen"), "logged: {log}");

    // A name set after both, drawn, so the sidebar has been drawn since.
    server.ok(&["set", "-t", &pane, "@tab_name", "after the phone"]);
    let screen = client.wait_text("after the phone");
    assert!(
        !screen
            .iter()
            .any(|l| l.contains("Phone:") || l.contains("from your phone")),
        "{}",
        screen.join("\n")
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
  set textContent(text) { this.replaceChildren(); this.text = text; }
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

/// A stand-in for the phone app in a tab of its own, marked as the phone
/// app's or not: a process that writes the app's lock with its pid and the
/// build given, then waits, run by a shell that goes on once it ends, as
/// `phone start`'s tab does. Gives its pane and the stand-in's pid.
fn stand_in_app(server: &TestServer, marked: bool, build: &str) -> (String, i64) {
    let lock = server.state.join("phone/lock");
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let _ = std::fs::remove_file(&lock);
    let app = server.dir.path().join("app.sh");
    std::fs::write(
        &app,
        format!(
            "printf '%s\\n%s\\n%s\\n' $$ '{build}' '0.0.0 (old)' > '{}'\nexec sleep 1000\n",
            lock.display()
        ),
    )
    .unwrap();
    let script = format!("sh '{}'; exec sh", app.display());
    let out = server.ok(&[
        "new-tab",
        "-d",
        "-n",
        "Phone app",
        "--",
        "sh",
        "-c",
        &script,
    ]);
    let pane = out.split_whitespace().nth(1).unwrap().to_string();
    if marked {
        server.ok(&["set", "-t", &pane, "@phone_app", "1"]);
    }
    wait_until(server, "the stand-in's lock", || {
        std::fs::read_to_string(&lock).is_ok_and(|t| t.lines().count() == 3)
    });
    let pid = std::fs::read_to_string(&lock)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    (pane, pid)
}

/// The build `phone run` would name for the server's binary.
fn build_of(bin: &std::path::Path) -> String {
    let bin = bin.canonicalize().unwrap();
    let modified = std::fs::metadata(&bin)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{} {modified}", bin.display())
}

fn alive(pid: i64) -> bool {
    // SAFETY: signal 0 only asks whether the process is there.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

fn pane_pid(server: &TestServer, pane: &str) -> Option<u64> {
    server.list()["sessions"]
        .as_array()?
        .iter()
        .flat_map(|s| s["tabs"].as_array().into_iter().flatten())
        .flat_map(|t| t["panes"].as_array().into_iter().flatten())
        .find(|p| p["id"] == pane)?["pid"]
        .as_u64()
}

fn upgrade(server: &TestServer) {
    let (code, out, err) = server.run(&["upgrade"]);
    assert_eq!(code, 0, "{out}{err}\nserver log: {}", server.log());
}

#[test]
fn an_upgrade_restarts_the_phone_app_from_another_build_on_its_own() {
    let (server, _bin) = TestServer::start_from_copy();
    common::write_settings(&server.home, json!({"phone": true}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (pane, old) = stand_in_app(&server, true, "/elsewhere/switchboard 1");
    let shell = pane_pid(&server, &pane);

    upgrade(&server);
    wait_until(&server, "the old app stopped", || !alive(old));
    wait_until(&server, "the pane started again", || {
        pane_pid(&server, &pane).is_some_and(|p| Some(p) != shell)
    });
    assert_eq!(
        server.ok(&["show", "-t", &pane, "@phone_app"]).trim(),
        "1",
        "still marked as the phone app's"
    );
    // `phone run` ran there, and with no phone.json didn't start, which
    // its log and the screen say.
    let log = server.state.join("phone/phone.log");
    wait_until(&server, "phone run's log line", || {
        std::fs::read_to_string(&log).is_ok_and(|t| t.contains("phone.json"))
    });
    assert!(
        server
            .log()
            .contains("the phone app was restarted on the new build"),
        "{}",
        server.log()
    );
    let deadline = common::deadline(Duration::from_secs(30));
    while !client
        .screen()
        .iter()
        .any(|l| l.contains("didn't start again on the new build"))
    {
        assert!(Instant::now() < deadline, "no message: {}", server.log());
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn an_upgrade_leaves_the_phone_app_alone_on_its_build_outside_its_tab_or_with_it_off() {
    let (server, bin) = TestServer::start_from_copy();
    common::write_settings(&server.home, json!({"phone": true}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let left_alone = |what: &str, pane: &str, pid: i64| {
        let shell = pane_pid(&server, pane);
        upgrade(&server);
        std::thread::sleep(Duration::from_secs(1) * common::patience());
        assert!(alive(pid), "{what}: the app was stopped\n{}", server.log());
        assert_eq!(pane_pid(&server, pane), shell, "{what}");
    };

    let (pane, pid) = stand_in_app(&server, true, &build_of(&bin));
    left_alone("on the server's build", &pane, pid);
    server.ok(&["kill-pane", "-t", &pane]);

    let (pane, pid) = stand_in_app(&server, false, "/elsewhere/switchboard 1");
    left_alone("in an unmarked tab", &pane, pid);
    server.ok(&["kill-pane", "-t", &pane]);

    common::write_settings(&server.home, json!({"phone": false}));
    let (pane, pid) = stand_in_app(&server, true, "/elsewhere/switchboard 1");
    left_alone("with the setting off", &pane, pid);
    assert!(!server.log().contains("restarted on the new build"));
}

/// The composer's field (assets/phone/app.js's `composer`), built in Node
/// over a stand-in document and synced for a message and for "your own
/// answer": plain message text that AutoFill leaves alone, with the
/// keyboard's corrections kept; skipped without Node.
#[test]
fn the_composer_is_message_text_autofill_leaves_alone() {
    let script = r#"
const vm = require("vm");
const fs = require("fs");
class Node {
  constructor(tag) { this.tag = tag; this.attrs = {}; this.kids = []; this.style = {}; this.value = ""; this.classList = { toggle() {} }; }
  setAttribute(k, v) { this.attrs[k] = String(v); }
  addEventListener() {}
  append(kid) { this.kids.push(kid); }
  replaceWith() {}
}
const ctx = vm.createContext({
  Node,
  document: { getElementById: () => null, addEventListener: () => {}, createElement: (tag) => new Node(tag), createTextNode: (t) => new Node("text") },
  window: { addEventListener: () => {} },
  fetch: () => new Promise(() => {}),
  console,
});
vm.runInContext(fs.readFileSync(process.argv[1], "utf8"), ctx);
const box = vm.runInContext("composer()", ctx);
const find = (n) => n.tag === "textarea" ? n : n.kids.map(find).find(Boolean);
for (const prompt of [null, { free: true }]) {
  box.sync({ index: 2, name: "notes", prompt });
  console.log(JSON.stringify(find(box).attrs));
}
"#;
    let Some(stdout) = run_app_js(script) else {
        return;
    };
    let lines: Vec<&str> = stdout.lines().collect();
    // A message to the tab, and "your own answer" to its question.
    assert_eq!(lines.len(), 2, "{stdout}");
    for line in lines {
        let attrs: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(attrs["autocomplete"], "off", "{line}");
        assert_eq!(attrs["name"], "message", "{line}");
        assert_eq!(attrs["autocorrect"], "on", "{line}");
        assert_eq!(attrs["autocapitalize"], "sentences", "{line}");
        assert_eq!(attrs["spellcheck"], "true", "{line}");
    }
}

/// The composer's bar (assets/phone/app.css's `.composer`) pads its text box
/// at the sides alone, so the bar is the text box's height, and the screen
/// leaves out the home-indicator inset while the keyboard is open.
#[test]
fn the_composer_is_its_text_boxs_height() {
    let css = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/phone/app.css"),
    )
    .unwrap();
    let rule = |selector: &str| {
        let start = css
            .find(&format!("\n{selector} {{"))
            .unwrap_or_else(|| panic!("no {selector} rule"));
        let body = &css[start..];
        body[..body.find('}').unwrap()].to_string()
    };
    assert!(
        rule(".composer").contains("padding: 0 12px;"),
        "{}",
        rule(".composer")
    );
    assert!(
        rule(".keyboard .screen").contains("padding-bottom: 0;"),
        "{}",
        rule(".keyboard .screen")
    );
}

/// The keyboard's class (assets/phone/app.js's `followKeyboard`), run in
/// Node over a stand-in window: set while the visual viewport is shorter than
/// the window, cleared as it grows back; skipped without Node.
#[test]
fn the_keyboard_open_is_told_from_the_visual_viewport() {
    let script = r#"
const vm = require("vm");
const fs = require("fs");
const classes = new Set();
const viewport = { height: 800, listeners: [], addEventListener(k, f) { if (k === "resize") this.listeners.push(f); } };
const ctx = vm.createContext({
  document: {
    getElementById: () => null,
    addEventListener: () => {},
    documentElement: { classList: { toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)) } },
  },
  window: { addEventListener: () => {}, innerHeight: 800, visualViewport: viewport },
  fetch: () => new Promise(() => {}),
  console,
});
vm.runInContext(fs.readFileSync(process.argv[1], "utf8"), ctx);
const resize = (height) => { viewport.height = height; for (const f of viewport.listeners) f(); };
console.log("closed=" + classes.has("keyboard"));
resize(470.5);
console.log("open=" + classes.has("keyboard"));
resize(800);
console.log("closed again=" + classes.has("keyboard"));
"#;
    let Some(stdout) = run_app_js(script) else {
        return;
    };
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        ["closed=false", "open=true", "closed again=false"],
        "{stdout}"
    );
}

/// What the phone's Close tab asks of the server: a Claude tab closed and
/// logged, with no notification; another pane refused.
#[test]
fn the_phone_closes_a_claude_tab_and_refuses_any_other() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-d", "-n", "plain"]);
    let plain = server.list()["sessions"][0]["tabs"][1]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (code, _, err) = server.run(&["kill-tab", "-t", &plain, "-s", "phone"]);
    assert_eq!(code, 1, "a pane not running Claude");
    assert!(err.contains("isn't running Claude"), "{err}");
    assert_eq!(
        server.list()["sessions"][0]["tabs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // Settings' "Close tabs from the phone" off refuses every close.
    common::write_settings(&server.home, json!({ "phone_close_tabs": false }));
    wait_until(&server, "the close refused while off", || {
        let (_, _, err) = server.run(&["kill-tab", "-t", &plain, "-s", "phone"]);
        err.contains("off in Settings")
    });
    common::write_settings(&server.home, json!({}));
    wait_until(&server, "the setting on again", || {
        let (_, _, err) = server.run(&["kill-tab", "-t", &plain, "-s", "phone"]);
        err.contains("isn't running Claude")
    });

    let pane = claude_pane(&server);
    let said = server.ok(&["kill-tab", "-t", &pane, "-s", "phone"]);
    assert!(said.starts_with("Closed tab 1 "), "{said}");
    let tabs = server.list()["sessions"][0]["tabs"].clone();
    assert_eq!(tabs.as_array().unwrap().len(), 1, "{tabs}");
    assert_eq!(tabs[0]["panes"][0]["id"], plain.as_str());
    assert!(
        server.log().contains("from your phone: tab 1 (") && server.log().contains("): closed it"),
        "logged: {}",
        server.log()
    );
}

/// The drawer's menu (assets/phone/app.js), run in Node over a page made
/// of plain objects: a long press on a row opens its tab's menu rather
/// than the tab, Close tab asks first, only Close closes it, and a tap
/// still opens the tab; skipped without Node.
#[test]
fn a_long_press_opens_a_tabs_menu_and_close_tab_asks_first() {
    let app = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/phone/app.js");
    let script = r#"
const vm = require("vm");
const fs = require("fs");
class Node {
  constructor(tag) { this.tag = tag; this.kids = []; this.attrs = {}; this.on = {}; this.className = ""; this.style = {}; this.parent = null; this.value = ""; }
  append(...kids) { for (const k of kids) { k.parent = this; this.kids.push(k); } }
  replaceChildren(...kids) { this.kids = []; this.append(...kids); }
  replaceWith(n) { const p = this.parent; if (p) { p.kids[p.kids.indexOf(this)] = n; n.parent = p; } }
  remove() { const p = this.parent; if (p) p.kids = p.kids.filter((k) => k !== this); this.parent = null; }
  setAttribute(k, v) { this.attrs[k] = v; }
  addEventListener(name, f) { (this.on[name] = this.on[name] || []).push(f); }
  removeEventListener() {}
  fire(name, e) {
    let stopped = false;
    const ev = Object.assign({ target: this, clientX: 0, clientY: 0, preventDefault() {}, stopPropagation() { stopped = true; } }, e);
    for (const f of doc.on[name] || []) f(ev);
    if (!stopped) for (const f of this.on[name] || []) f(ev);
  }
  contains(n) { for (; n; n = n.parent) if (n === this) return true; return false; }
  querySelector() { return null; }
  get classList() { const n = this; return { toggle(c, on) { const s = new Set(n.className.split(" ").filter(Boolean)); on ? s.add(c) : s.delete(c); n.className = [...s].join(" "); } }; }
  get text() { return this.kids.map((k) => (k instanceof Node ? k.text : k.data)).join(""); }
  all(pred) { const out = []; const walk = (n) => { if (!(n instanceof Node)) return; if (pred(n)) out.push(n); n.kids.forEach(walk); }; walk(this); return out; }
}
const app = new Node("div"), body = new Node("body");
const doc = { on: {} };
const timers = [];
const posted = [];
const ctx = vm.createContext({
  Node,
  document: {
    getElementById: () => app,
    body,
    createElement: (t) => new Node(t),
    createTextNode: (data) => ({ data }),
    addEventListener: (name, f) => { (doc.on[name] = doc.on[name] || []).push(f); },
    removeEventListener() {},
  },
  window: { innerHeight: 800, addEventListener() {} },
  history: { replaceState() {} },
  setTimeout: (f) => { timers.push(f); return timers.length; },
  clearTimeout: (id) => { timers[id - 1] = null; },
  fetch: (path, opts) => {
    if (opts && opts.method === "POST") { posted.push(path); return Promise.resolve({ ok: true, status: 200, json: async () => ({ said: "Closed tab 4 full titles" }) }); }
    return new Promise(() => {});
  },
  console,
});
vm.runInContext(fs.readFileSync(process.argv[1], "utf8"), ctx);
vm.runInContext(`
  state.me = { paired: true, computer: "studio" };
  state.tabs = [
    { index: 4, pane: "%4", name: "full titles", state: "done", needs: true },
    { index: 3, pane: "%3", name: "M-d uncommitted", state: "working" },
  ];
  render();
`, ctx);
const row = (n) => app.all((x) => x.attrs["data-pane"] === n)[0];
const options = () => body.all((x) => x.attrs.role === "menuitem").map((x) => x.text);
const button = (label) => body.all((x) => x.tag === "button" && x.text === label)[0];
const out = [];
(async () => {
  // A long press: the menu, and no tab opened by the click after it.
  row("4").fire("pointerdown");
  timers.splice(0).forEach((f) => f && f());
  row("4").fire("pointerup");
  row("4").fire("click");
  out.push("menu=" + options().join(",") + " open=" + vm.runInContext("state.open", ctx));
  // Close tab asks first; No leaves the tab.
  button("Close tab").fire("click");
  out.push("asks=" + body.text.includes("Close tab 4 full titles? [Y/n]") + " posted=" + posted.length);
  const answers = body.all((x) => x.className === "answers")[0].kids.map((k) => k.text + (k.className.includes("lit") ? "*" : ""));
  out.push("answers=" + answers.join(","));
  button("No").fire("click");
  out.push("no=" + posted.length + " menu=" + vm.runInContext("state.menu", ctx));
  // Asked again, Close closes it.
  row("4").fire("contextmenu");
  button("Close tab").fire("click");
  button("Close").fire("click");
  await new Promise((r) => setImmediate(r));
  out.push("closed=" + posted.join(",") + " tabs=" + vm.runInContext("state.tabs.map((t) => t.index).join()", ctx));
  // On a tab still working, Close tab alone; a tap opens the tab.
  row("3").fire("contextmenu");
  out.push("working=" + options().join(","));
  button("×").fire("click");
  row("3").fire("pointerdown");
  row("3").fire("pointerup");
  row("3").fire("click");
  out.push("tap=" + vm.runInContext("state.open", ctx));
  // A redraw while a finger is down: lifting it still lets the hold go.
  vm.runInContext("state.open = null; state.tabs.push({ index: 5, pane: \"%5\", name: \"five\", state: \"idle\" }); render();", ctx);
  row("5").fire("pointerdown");
  vm.runInContext("render()", ctx);
  row("5").fire("pointerup");
  timers.splice(0).forEach((f) => f && f());
  out.push("redrawn=" + vm.runInContext("state.menu", ctx));
  // A tap on the lit row, outside the menu, closes it.
  row("5").fire("contextmenu");
  row("5").fire("pointerdown");
  row("5").fire("pointerup");
  row("5").fire("click");
  out.push("lit row=" + vm.runInContext("state.menu", ctx) + " open=" + vm.runInContext("state.open", ctx));
  // Close tabs from the phone off: a tab still working has nothing to offer.
  vm.runInContext("state.close = false", ctx);
  row("3").fire("contextmenu");
  out.push("off=" + vm.runInContext("state.menu", ctx) + " " + options().join(","));
  console.log(out.join("\n"));
})();
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
            "menu=Mark seen,Close tab open=null",
            "asks=true posted=0",
            "answers=Close*,No",
            "no=0 menu=null",
            "closed=/api/tabs/4/close tabs=3",
            "working=Close tab",
            "tap=3",
            "redrawn=null",
            "lit row=null open=null",
            "off=null ",
        ],
        "{stdout}"
    );
}

/// The tab's name at the top of a channel (assets/phone/app.js's `title`),
/// run in Node over a stand-in for the page; skipped without Node.
#[test]
fn a_tap_on_a_long_tab_name_shows_it_whole_until_another_tab_opens() {
    let app = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/phone/app.js");
    // Only what `el` and `title` touch of the page, each element as wide as
    // its test says, and the page's start left waiting on a fetch that never
    // answers.
    let script = r#"
const vm = require("vm");
const fs = require("fs");
class Node { constructor() { this.kids = []; } append(...k) { this.kids.push(...k); } }
class Element extends Node {
  constructor() { super(); this.attrs = {}; this.on = {}; this.className = ""; this.scrollWidth = 0; this.clientWidth = 0; }
  setAttribute(k, v) { this.attrs[k] = v; }
  getAttribute(k) { return this.attrs[k]; }
  addEventListener(k, f) { this.on[k] = f; }
}
const ctx = vm.createContext({
  Node,
  document: {
    getElementById: () => null,
    addEventListener: () => {},
    createElement: () => new Element(),
    createTextNode: (t) => Object.assign(new Node(), { text: t }),
  },
  window: { addEventListener: () => {} },
  history: { replaceState: () => {} },
  location: { hash: "" },
  fetch: () => new Promise(() => {}),
  console,
});
vm.runInContext(fs.readFileSync(process.argv[1], "utf8"), ctx);
vm.runInContext(`
  var button;
  var draws = 0;
  state.tabs = [
    { index: 2, pane: "%2", name: "a name far too long for the one line at the top of the phone", state: "idle" },
    { index: 3, pane: "%3", name: "short", state: "idle" },
  ];
  render = () => { draws++; button = title(state.tabs[0]); };
  var look = () => button.className + " " + button.getAttribute("aria-expanded");
  var tap = (wide, room) => { button.scrollWidth = wide; button.clientWidth = room; button.on.click(); };
`, ctx);
const run = (code) => console.log(vm.runInContext(code, ctx));
run(`button = title(state.tabs[0]); look()`);
// A name that fits: nothing changes.
run(`tap(100, 100); look() + " " + draws`);
run(`tap(300, 100); look()`);
// A redraw, as when a message arrives, keeps it whole.
run(`render(); look()`);
run(`tap(100, 100); look()`);
run(`tap(300, 100); openTab(number(state.tabs[1])); button = title(state.tabs[1]); look()`);
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
            "title false",
            "title false 0",
            "title whole true",
            "title whole true",
            "title false",
            "title false",
        ],
        "{stdout}"
    );
}

/// A tapped push (assets/phone/sw.js's `notificationclick`) and the page
/// asking for its tab as it starts, comes back or is told (assets/phone/app.js's
/// `start`, `cameBack` and `offerPush`),
/// the worker and the page each run in Node with the browser stood in for;
/// skipped without Node.
#[test]
fn a_tapped_pushs_tab_opens_once_as_the_page_comes_back() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/phone");
    let script = r#"
const vm = require("vm");
const fs = require("fs");
const path = require("path");
const later = (f, ms) => { const t = setTimeout(f, ms); t.unref(); return t; };
const settle = () => new Promise((r) => setTimeout(r, 50));

// The worker: its caches, clients and events stood in for.
const listeners = {};
const stores = new Map();
const opened = [];
const sent = [];
let windows = [];
const worker = vm.createContext({
  self: {
    addEventListener: (type, fn) => (listeners[type] = fn),
    location: { origin: "https://phone" },
    clients: {
      matchAll: async () => windows,
      openWindow: async (url) => opened.push(url),
    },
  },
  caches: {
    open: async (name) => {
      if (!stores.has(name)) stores.set(name, new Map());
      const store = stores.get(name);
      return {
        put: async (key, res) => store.set(key, await res.text()),
        match: async (key) => (store.has(key) ? new Response(store.get(key)) : undefined),
        delete: async (key) => store.delete(key),
      };
    },
    keys: async () => [...stores.keys()],
  },
  Response,
  console,
});
vm.runInContext(fs.readFileSync(path.join(process.argv[1], "sw.js"), "utf8"), worker);
const tap = async (tab) => {
  const waits = [];
  listeners.notificationclick({ notification: { close() {}, data: { tab } }, waitUntil: (p) => waits.push(p) });
  await Promise.all(waits);
};

// The page, its worker the one above.
let now = Date.now();
let tabsFail = false;
const pageListeners = {};
const controller = {
  postMessage: (data, ports) => listeners.message({ data, ports: ports || [], waitUntil: () => {} }),
};
const serviceWorker = {
  controller,
  register: async () => ({}),
  addEventListener: (type, fn) => (pageListeners[type] = fn),
};
const answer = (body) => Promise.resolve({ ok: true, status: 200, json: async () => body });
const page = vm.createContext({
  document: { getElementById: () => null, addEventListener: () => {}, visibilityState: "visible" },
  window: { addEventListener: () => {}, PushManager: {} },
  location: { hash: "" },
  history: { replaceState: () => {} },
  navigator: { serviceWorker },
  Notification: { permission: "denied" },
  fetch: (url) =>
    url === "/api/me"
      ? answer({ paired: true })
      : url === "/api/tabs"
        ? tabsFail
          ? Promise.reject(new Error("offline"))
          : answer({ live: true, tabs: [{ pane: "%7" }, { pane: "%9" }] })
        : new Promise(() => {}),
  MessageChannel,
  setTimeout: later,
  clearTimeout,
  console,
});
vm.runInContext(fs.readFileSync(path.join(process.argv[1], "app.js"), "utf8"), page);
vm.runInContext(`render = () => {}; listen = () => {}; loadTurns = async () => {};`, page);
vm.runInContext(`Date.now = () => globalThis.now();`, page);
page.now = () => now;
const openIn = async (open, run) => {
  vm.runInContext(`state.open = ${JSON.stringify(open)};`, page);
  run();
  await settle();
  return String(vm.runInContext("state.open", page));
};
const backOn = (open) => openIn(open, () => page.cameBack());

(async () => {
  await settle(); // the page's own start, with nothing tapped
  // Closed, and the tabs not read as it starts: the tab waits for the next
  // coming back rather than being lost.
  await tap("7");
  console.log("opened=" + opened.join(","));
  tabsFail = true;
  console.log("unread=" + (await openIn("9", () => page.start())));
  tabsFail = false;
  console.log("back=" + (await backOn("9")));
  // Taken once: coming back again stays where it is.
  console.log("again=" + (await backOn("9")));
  // In the background: the window focused, told, and sent to the tab.
  const told = [];
  let posted = null;
  windows = [{ focus: async () => {}, postMessage: (m) => told.push((posted = m).open), navigate: async (url) => told.push(url) }];
  await tap("7");
  console.log("told=" + told.join(","));
  // The page running: the message opens it, and coming back after doesn't again.
  console.log("message=" + (await openIn("9", () => pageListeners.message({ data: posted }))));
  console.log("after=" + (await backOn("9")));
  // A page the worker doesn't control: the tab the message names, which the
  // worker, asked later, doesn't hand out again.
  await tap("7");
  serviceWorker.controller = null;
  console.log("uncontrolled=" + (await openIn("9", () => pageListeners.message({ data: posted }))));
  serviceWorker.controller = controller;
  console.log("afterUncontrolled=" + (await backOn("9")));
  // Coming back with the tabs not read: left, never taken against the tabs
  // held, and opened once the stream brings them.
  await tap("7");
  tabsFail = true;
  console.log("failedBack=" + (await backOn("9")));
  tabsFail = false;
  console.log("stream=" + (await openIn("9", () => vm.runInContext("tabsRead()", page))));
  // Tapped over a minute ago: left alone.
  await tap("7");
  now += 61000;
  console.log("old=" + (await backOn("9")));
  // A tab since closed: the drawer.
  now = Date.now();
  await tap("5");
  console.log("closed=" + (await backOn("9")));
})();
"#;
    let out = match std::process::Command::new("node")
        .arg("-e")
        .arg(script)
        .arg(&dir)
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
            "opened=/#7",
            "unread=9",
            "back=7",
            "again=9",
            "told=7,/#7",
            "message=7",
            "after=9",
            "uncontrolled=7",
            "afterUncontrolled=9",
            "failedBack=9",
            "stream=7",
            "old=9",
            "closed=null",
        ],
        "{stdout}"
    );
}

// ── `switchboard phone preview` (src/phone/preview.rs) ──

/// Behind the `net` feature, as the preview is.
#[cfg(feature = "net")]
mod preview_server {
    use super::*;

    /// A preview serving `dir`, in a runtime of its own; where it listens.
    fn preview(dir: &std::path::Path) -> std::net::SocketAddr {
        let (tx, rx) = std::sync::mpsc::channel();
        let dir = dir.to_path_buf();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let listener = switchboard::phone::preview::bind(Some(0)).await.unwrap();
                tx.send(listener.local_addr().unwrap()).unwrap();
                let _ = switchboard::phone::preview::serve(listener, dir).await;
            });
        });
        rx.recv().unwrap()
    }

    /// A request to the preview: its status and body.
    fn http(
        at: std::net::SocketAddr,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, String) {
        use std::io::Read;
        let mut stream = std::net::TcpStream::connect(at).unwrap();
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: {at}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut out = String::new();
        stream.read_to_string(&mut out).unwrap();
        let status = out[9..12].parse().unwrap();
        let body = out
            .split_once("\r\n\r\n")
            .map_or("", |(_, b)| b)
            .to_string();
        (status, body)
    }

    fn json_of(at: std::net::SocketAddr, path: &str) -> Value {
        let (status, body) = http(at, "GET", path, None);
        assert_eq!(status, 200, "{path}: {body}");
        serde_json::from_str(&body).unwrap()
    }

    fn assets() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/phone")
    }

    #[test]
    fn the_preview_serves_the_app_and_made_up_tabs_on_loopback_alone() {
        let at = preview(&assets());
        assert_eq!(at.ip(), std::net::Ipv4Addr::LOCALHOST);
        let (status, page) = http(at, "GET", "/", None);
        assert_eq!(status, 200);
        assert!(page.contains("/app.js"), "{page}");
        let (_, css) = http(at, "GET", "/app.css", None);
        assert!(css.contains("var(--safe-area-inset-top, 0px)"));
        assert!(!css.contains("env(safe-area-inset"));
        let (_, frame) = http(at, "GET", "/preview", None);
        assert!(frame.contains("<iframe"), "{frame}");
        assert_eq!(http(at, "GET", "/sw.js", None).0, 404, "no service worker");
        assert_eq!(json_of(at, "/api/me")["paired"], true);
        let tabs = json_of(at, "/api/tabs");
        assert_eq!(tabs["live"], true);
        assert_eq!(tabs["tabs"].as_array().unwrap().len(), 12);
        let turns = json_of(at, "/api/tabs/13/turns");
        assert!(!turns["messages"].as_array().unwrap().is_empty());
    }

    #[test]
    fn an_answer_clears_the_made_up_question_and_offline_holds_until_back() {
        let at = preview(&assets());
        let tab = |tabs: &Value| {
            tabs["tabs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["pane"] == "%11")
                .cloned()
                .unwrap()
        };
        let before = tab(&json_of(at, "/api/tabs"));
        assert_eq!(before["state"], "waiting");
        let id = before["prompt"]["id"].clone();
        let (status, body) = http(
            at,
            "POST",
            "/api/tabs/11/answer",
            Some(json!({"reply": "1", "prompt": id})),
        );
        assert_eq!(status, 200, "{body}");
        let after = tab(&json_of(at, "/api/tabs"));
        assert!(after["prompt"].is_null(), "{after}");
        assert_eq!(after["state"], "working");
        let turns = json_of(at, "/api/tabs/11/turns");
        let last = turns["messages"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(
            (last["who"].as_str(), last["text"].as_str()),
            (Some("you"), Some("Yes"))
        );
        // The question gone, the same answer is refused.
        let (status, _) = http(
            at,
            "POST",
            "/api/tabs/11/answer",
            Some(json!({"reply": "1", "prompt": id})),
        );
        assert_eq!(status, 409);

        assert_eq!(http(at, "POST", "/preview/offline", None).0, 200);
        assert_eq!(json_of(at, "/api/tabs")["live"], false);
        let (status, _) = http(
            at,
            "POST",
            "/api/tabs/16/message",
            Some(json!({"text": "hello", "images": []})),
        );
        assert_eq!(status, 503);
        assert_eq!(http(at, "POST", "/preview/online", None).0, 200);
        assert_eq!(json_of(at, "/api/tabs")["live"], true);
    }

    #[test]
    fn a_saved_file_sends_a_reload() {
        use std::io::{BufRead, BufReader};
        let dir = common::TempDir::new("sb-phone-preview");
        for entry in std::fs::read_dir(assets()).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
        }
        let at = preview(dir.path());
        let mut stream = std::net::TcpStream::connect(at).unwrap();
        write!(
            stream,
            "GET /preview/reload HTTP/1.1\r\nHost: {at}\r\nAccept: text/event-stream\r\n\r\n"
        )
        .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        // The headers read, the stream is open before the file is saved.
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
        }
        let css = dir.path().join("app.css");
        let mut text = std::fs::read_to_string(&css).unwrap();
        text.push_str("\n.saved { color: red; }\n");
        std::fs::write(&css, text).unwrap();
        stream
            .set_read_timeout(Some(
                common::deadline(Duration::from_secs(5)).saturating_duration_since(Instant::now()),
            ))
            .unwrap();
        loop {
            line.clear();
            reader
                .read_line(&mut line)
                .expect("a reload before the deadline");
            if line.starts_with("data:") {
                assert_eq!(line.trim(), "data: app.css");
                break;
            }
        }
    }
}
