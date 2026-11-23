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

/// The server's `watch`, as the phone app follows it.
fn watch(server: &TestServer) -> UnixStream {
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
    stream
}

#[test]
fn watch_streams_the_claude_tabs_as_they_change() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-d", "-n", "plain"]);
    let pane = claude_pane(&server);

    let mut stream = watch(&server);
    let first = next_line(&mut stream, |v| v["tabs"].as_array().is_some());
    let tabs = first["tabs"].as_array().unwrap();
    assert_eq!(tabs.len(), 1, "the Claude tab alone: {first}");
    assert_eq!(tabs[0]["pane"], pane);
    assert_eq!(tabs[0]["needs"], false);
    // Its group, badge and words as the sidebar's model gives them.
    assert_eq!(tabs[0]["stage"], "idle", "{first}");
    assert_eq!(tabs[0]["group"], "Idle");
    assert_eq!(tabs[0]["mark"], "○");
    assert_eq!(tabs[0]["spins"], false);
    assert!(tabs[0]["words"].is_string());

    show_prompt(&server, &pane);
    server.ok(&["set", "-t", &pane, "@agent_state", "waiting"]);
    let waiting = next_line(&mut stream, |v| v["tabs"][0]["state"] == "waiting");
    assert_eq!(waiting["tabs"][0]["needs"], true);
    assert_eq!(waiting["tabs"][0]["stage"], "needs");
    assert_eq!(waiting["tabs"][0]["mark"], "●");
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

/// Each tab the phone reads carries its Claude's address, which names a
/// message another tab's Claude sent in a channel (board 182).
#[test]
fn the_tabs_the_phone_reads_carry_their_claudes_address() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0].clone();
    let pid = pane["pid"].as_u64().unwrap();
    let dir = server.home.join(".claude/sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let file = json!({"pid": pid, "entrypoint": "cli", "sessionId": "s-phone", "messagingSocketPath": "/tmp/cc-socks/tab-one.sock"});
    std::fs::write(dir.join(format!("{pid}.json")), file.to_string()).unwrap();

    let mut stream = watch(&server);
    let seen = next_line(&mut stream, |v| {
        v["tabs"][0]["address"] == "uds:/tmp/cc-socks/tab-one.sock"
    });
    assert_eq!(seen["tabs"][0]["pane"], pane["id"], "{seen}");
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

/// A page of its own for assets/phone/app.js's channel, in Node: a stand-in
/// for the DOM's few parts the app uses, `app` its root, and the app loaded
/// into `ctx`, its stream in `source`.
const CHANNEL_PAGE: &str = r##"
const vm = require("vm");
const fs = require("fs");
class Node {
  constructor(tag) {
    this.tagName = tag; this.children = []; this.parentNode = null; this.className = "";
    this.listeners = {}; this.options = {}; this.style = {}; this.value = ""; this.text = "";
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
  addEventListener(k, f, options) { (this.listeners[k] = this.listeners[k] || []).push(f); this.options[k] = options; }
  // Sent to this node, keeping it as the target, and up through its parents.
  dispatchEvent(e) { e.target = e.target || this; for (let n = this; n; n = n.parentNode) for (const f of n.listeners[e.type] || []) f(e); }
  click() { this.dispatchEvent(new Event("click")); }
  focus() { document.activeElement = this; }
  append(...kids) { for (const k of kids) { if (k.parentNode) k.remove(); k.parentNode = this; this.children.push(k); } }
  remove() { const p = this.parentNode; if (p) { p.children = p.children.filter((c) => c !== this); this.parentNode = null; } }
  replaceChildren(...kids) { for (const c of this.children) c.parentNode = null; this.children = []; this.append(...kids); }
  replaceWith(n) { const p = this.parentNode; const i = p.children.indexOf(this); if (n.parentNode) n.remove(); p.children[i] = n; n.parentNode = p; this.parentNode = null; }
  insertBefore(n, ref) { if (n.parentNode) n.remove(); n.parentNode = this; this.children.splice(this.children.indexOf(ref), 0, n); }
  // Selectors of "tag.class" parts, separated by commas.
  matches(sel) { return sel.split(",").some((part) => { const [tag, ...classes] = part.trim().split("."); return (!tag || this.tagName === tag) && classes.every((c) => this.classList.has(c)); }); }
  closest(sel) { for (let n = this; n; n = n.parentNode) if (n.matches && n.matches(sel)) return n; return null; }
  contains(n) { for (; n; n = n.parentNode) if (n === this) return true; return false; }
  all() { return this.children.flatMap((c) => [c, ...c.all()]); }
  // Selectors of "tag.class" parts, each part a descendant of the one before;
  // an array without `find`, which a page's NodeList lacks.
  querySelectorAll(sel) {
    let found = [this];
    for (const part of sel.split(" ")) {
      const [tag, ...classes] = part.split(".");
      found = [...new Set(found.flatMap((n) => n.all()))].filter(
        (n) => (!tag || n.tagName === tag) && classes.every((c) => n.classList.has(c)),
      );
    }
    return Object.assign(found, { find: undefined });
  }
  querySelector(sel) { return this.querySelectorAll(sel)[0] || null; }
}
class Event {
  constructor(type, init) { Object.assign(this, init); this.type = type; this.defaultPrevented = false; }
  preventDefault() { this.defaultPrevented = true; }
}
const app = new Node("div");
const document = {
  activeElement: null,
  getElementById: () => app,
  addEventListener: () => {},
  createElement: (tag) => new Node(tag),
  createTextNode: (text) => Object.assign(new Node("#text"), { text }),
};
let source = null;
const addresses = [];
let buzzes = 0;
const ctx = vm.createContext({
  document, Node, Event, console,
  navigator: { vibrate: () => buzzes++ },
  EventSource: class { constructor() { source = this; } },
  history: { replaceState: (_, __, url) => addresses.push(url) },
  window: { addEventListener: () => {}, innerWidth: 390 },
  setTimeout,
  clearTimeout,
  location: { hash: "" },
  fetch: () => new Promise(() => {}),
});
vm.runInContext(fs.readFileSync(process.argv[1], "utf8"), ctx);
"##;

/// An open tab's channel (assets/phone/app.js's `channel`), run in Node over
/// a page of its own: an update from the stream, and new turns, leave the
/// composer's field in place, focused and with what was typed, and a message
/// opened with Show all open; skipped without Node.
#[test]
fn an_update_keeps_the_composer_and_the_messages_opened() {
    let script = [CHANNEL_PAGE, r##"
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
"##]
    .concat();
    let Some(stdout) = run_app_js(&script) else {
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

/// A working tab's composer (assets/phone/app.js's `composer`), run in
/// Node over a page of its own: empty, its button is Stop (■), which asks
/// first and stops the turn only on Stop; with words typed it is Send
/// again, which sends them as a message; on an idle tab it is Send;
/// skipped without Node.
#[test]
fn a_working_tabs_empty_composer_stops_it_after_asking() {
    let script = [CHANNEL_PAGE, r##"
document.body = new Node("body");
document.removeEventListener = () => {};
const posted = [];
ctx.fetch = (path, opts) => {
  if (opts && opts.method === "POST") posted.push(path + " " + opts.body);
  return Promise.resolve({ ok: true, status: 200, json: async () => ({ said: "Stopped tab 2 phone" }) });
};
const tab = (state) => ({ pane: "%5", index: 2, name: "phone", state, wait: "", turn: { prompt: "go" } });
vm.runInContext(`
  state.me = { paired: true, computer: "mac" };
  listen();
  state.open = "5";
`, ctx);
const update = (t) => source.onmessage({ data: JSON.stringify({ live: true, tabs: [t] }) });
const send = () => app.querySelector(".composer button.send");
const look = () => send().textContent + " " + send()["aria-label"];
const button = (label) => document.body.all().find((x) => x.tagName === "button" && x.textContent === label);
const tick = () => new Promise((r) => setImmediate(r));
const out = [];
(async () => {
  update(tab("working"));
  out.push("empty=" + look());
  send().click();
  out.push("asks=" + document.body.querySelector(".question").textContent);
  button("No").click();
  await tick();
  out.push("no=" + posted.length);
  send().click();
  button("Stop").click();
  await tick();
  out.push("stop=" + posted.join(" | "));
  const field = app.querySelector(".composer textarea");
  field.value = "after that, run the tests";
  field.dispatchEvent(new Event("input"));
  out.push("typed=" + look());
  send().click();
  await tick();
  out.push("sent=" + posted[posted.length - 1]);
  update(tab("idle"));
  out.push("idle=" + look());
  console.log(out.join("\n"));
})();
"##]
    .concat();
    let Some(stdout) = run_app_js(&script) else {
        return;
    };
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "empty=■ Stop",
            "asks=Stop tab 2's Claude? [Y/n]",
            "no=0",
            "stop=/api/tabs/5/stop {}",
            "typed=➤ Send",
            r#"sent=/api/tabs/5/message {"text":"after that, run the tests","images":[]}"#,
            "idle=➤ Send",
        ],
        "{stdout}"
    );
}

/// A channel scrolled to its top (assets/phone/app.js's `loadEarlier`)
/// asks for the turns before the earliest shown, says "Loading earlier…"
/// meanwhile, inserts them above with what was on screen kept in place,
/// says "Start of the conversation" once it has the first, and keeps them
/// as the newest turns are read again; skipped without Node.
#[test]
fn scrolling_to_a_channels_top_loads_the_turns_before() {
    let script = [CHANNEL_PAGE, r##"
const turn = (n) => [
  { who: "you", text: "ask " + n, at: n * 10, offset: n * 10 },
  { who: "claude", text: "reply " + n, at: n * 10 + 1, offset: n * 10 + 5 },
];
const turns = (a, b) => Array.from({ length: b - a }, (_, i) => turn(a + i)).flat();
let latest = { messages: turns(20, 40), from: 200, earlier: true, conversation: "c" };
const asked = [];
ctx.fetch = (path) => {
  asked.push(path);
  const body = path.includes("before=200") ? { messages: turns(0, 20), from: 0, earlier: false, conversation: "c" } : latest;
  return Promise.resolve({ status: 200, ok: true, json: async () => body });
};
const tick = () => new Promise((r) => setTimeout(r, 0));
(async () => {
  vm.runInContext(`
    state.me = { paired: true, computer: "mac" };
    listen();
    state.open = "5";
  `, ctx);
  source.onmessage({ data: JSON.stringify({ live: true, tabs: [{ pane: "%5", index: 2, name: "phone", state: "idle", turn: { prompt: "go" } }] }) });
  const list = app.querySelector(".channel");
  Object.defineProperty(list, "scrollHeight", { get: () => app.querySelectorAll(".msg").length * 50 });
  await vm.runInContext("loadTurns()", ctx);
  const first = () => app.querySelector(".msg .body").textContent;
  console.log("opened=" + app.querySelectorAll(".msg").length + " " + first() + " at " + list.scrollTop);
  console.log("above=" + JSON.stringify(app.querySelector(".above").textContent));
  list.scrollTop = 0;
  list.dispatchEvent(new Event("scroll"));
  console.log("above=" + app.querySelector(".above").textContent);
  for (let i = 0; i < 5; i++) await tick();
  console.log("asked=" + asked.join(" "));
  console.log("loaded=" + app.querySelectorAll(".msg").length + " " + first() + " at " + list.scrollTop);
  console.log("above=" + app.querySelector(".above").textContent);
  latest = { messages: turns(21, 41), from: 210, earlier: true, conversation: "c" };
  await vm.runInContext("loadTurns()", ctx);
  console.log("kept=" + app.querySelectorAll(".msg").length + " " + first());
  // More new turns than a read holds: no gap kept, the channel starts again.
  latest = { messages: turns(70, 90), from: 700, earlier: true, conversation: "c" };
  await vm.runInContext("loadTurns()", ctx);
  console.log("again=" + app.querySelectorAll(".msg").length + " " + first());
})();
"##]
    .concat();
    let Some(stdout) = run_app_js(&script) else {
        return;
    };
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "opened=40 ask 20 at 2000",
            "above=\"\"",
            "above=Loading earlier…",
            "asked=/api/tabs/5/turns /api/tabs/5/turns?before=200&until=200",
            "loaded=80 ask 0 at 2000",
            "above=Start of the conversation",
            "kept=82 ask 0",
            "again=40 ask 70",
        ],
        "{stdout}"
    );
}

/// Pasting into the composer (assets/phone/app.js's `composer`) adds the
/// clipboard's PNG, JPEG, GIF and WebP images to the ones waiting over it,
/// as + does, leaving its text to paste as the browser would, and adds none
/// while a question takes an answer of your own; skipped without Node.
#[test]
fn pasting_an_image_adds_it_over_the_composer() {
    let script = [CHANNEL_PAGE, r##"
ctx.URL = { createObjectURL: (f) => "blob:" + f.name, revokeObjectURL: () => {} };
const tab = (prompt) => ({ pane: "%5", index: 2, name: "phone", state: "idle", prompt, turn: { prompt: "go" } });
vm.runInContext(`
  state.me = { paired: true, computer: "mac" };
  listen();
  state.open = "5";
`, ctx);
const update = (t) => source.onmessage({ data: JSON.stringify({ live: true, tabs: [t] }) });
update(tab(null));
const file = (name, type) => ({ kind: "file", type, getAsFile: () => ({ name, type }) });
const paste = () => {
  const e = new Event("paste", { clipboardData: { items: [
    file("shot.png", "image/png"),
    file("doc.pdf", "application/pdf"),
    { kind: "string", type: "text/plain", getAsFile: () => null },
  ] } });
  app.querySelector(".composer textarea").dispatchEvent(e);
  return e;
};
const e = paste();
console.log("text pasted=" + !e.defaultPrevented);
console.log("shots=" + app.querySelectorAll(".tray .shot").length);
console.log("src=" + app.querySelector(".tray .shot img").src);
update(tab({ free: true, options: [] }));
paste();
console.log("answering=" + vm.runInContext("state.chosen.length", ctx));
"##]
    .concat();
    let Some(stdout) = run_app_js(&script) else {
        return;
    };
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "text pasted=true",
            "shots=1",
            "src=blob:shot.png",
            "answering=1"
        ],
        "{stdout}"
    );
}

/// A turn's steps folded into one row (assets/phone/app.js's `turns`): a
/// turn of 12 tool calls with text between them and a final reply shows what
/// you said, `▸ 12 steps` and the reply; a tap opens the steps in order, kept
/// open across a redraw, and a second folds them; a working turn's row shows
/// its latest step; a turn with only a reply has no row. Skipped without Node.
#[test]
fn a_turns_steps_fold_into_one_row() {
    let script = [CHANNEL_PAGE, r##"
const tab = (state) => ({ pane: "%5", index: 2, name: "phone", state, turn: { prompt: "go" } });
vm.runInContext(`
  state.me = { paired: true, computer: "mac" };
  listen();
  state.open = "5";
`, ctx);
const update = (t) => source.onmessage({ data: JSON.stringify({ live: true, tabs: [t] }) });
const steps = [];
for (let i = 1; i <= 12; i++) {
  steps.push({ who: "tool", text: "Bash: step " + i, at: 10 + i });
  if (i === 6) steps.push({ who: "claude", text: "Halfway.", at: 16 });
}
const done = [
  { who: "you", text: "Yep", at: 1 },
  ...steps,
  { who: "claude", text: "I filed the issue.", at: 30 },
  { who: "you", text: "Thanks", at: 40 },
  { who: "claude", text: "Anytime.", at: 41 },
];
const draw = (messages) => vm.runInContext("state.messages = " + JSON.stringify(messages) + "; render(true);", ctx);
const said = () => app.querySelectorAll("div.msg").map((m) => {
  const row = m.querySelector("button.steps");
  if (row) return "[" + row.textContent + " " + row["aria-expanded"] + "]";
  const tool = m.querySelector("div.tool");
  if (tool) return "tool:" + tool.textContent.replace("Bash: ", "");
  return m.querySelector("div.body").textContent.trim();
}).join(" | ");
update(tab("done"));
draw(done);
console.log("folded=" + said());
app.querySelector("button.steps").click();
const opened = said();
console.log("opened=" + opened);
console.log("focused=" + (document.activeElement === app.querySelector("button.steps")));
update(tab("idle"));
draw([...done]);
console.log("kept=" + (said() === opened));
app.querySelector("button.steps").click();
console.log("folded again=" + said());
update(tab("working"));
draw([{ who: "you", text: "Go", at: 50 }, { who: "tool", text: "Read app.js", at: 51 }, { who: "claude", text: "Now the tests.", at: 52 }, { who: "tool", text: "Bash: cargo test", at: 53 }]);
console.log("working=" + said());
// A turn longer than the window the computer sends, opened, stays open as
// the window slides past its first steps.
const window = (from) => Array.from({ length: 5 }, (_, i) => ({ who: "tool", text: "Bash: step " + (from + i), at: 100 + from + i }));
draw([{ who: "you", text: "Long", at: 99 }, ...window(1)]);
app.querySelector("button.steps").click();
draw(window(1));
console.log("left=" + said());
draw(window(2));
console.log("slid=" + said());
// A newer turn, once its message leaves too, takes none of that open state.
draw([...window(2), { who: "you", text: "Next", at: 200 }, { who: "tool", text: "Bash: next", at: 201 }]);
console.log("next=" + said());
// The phone's copy of a message sent from it, then the computer's.
vm.runInContext("state.messages.push({ who: \"you\", text: \"Then\", at: 300, sent: true }); render(true);", ctx);
draw([...window(3), { who: "you", text: "Next", at: 200 }, { who: "tool", text: "Bash: next", at: 201 }, { who: "you", text: "Then", at: 301 }]);
console.log("sent=" + said());
draw([{ who: "tool", text: "Bash: next", at: 201 }, { who: "tool", text: "Bash: after", at: 202 }]);
console.log("newer=" + said());
// A tab in the background (a Monitor, a /loop between runs) has ended its turn.
update(tab("background"));
draw([{ who: "you", text: "Go", at: 50 }, { who: "tool", text: "Monitor: CI", at: 51 }, { who: "claude", text: "Watching CI.", at: 52 }]);
console.log("background=" + said());
draw([{ who: "you", text: "Hi", at: 60 }, { who: "claude", text: "Hello.", at: 61 }]);
console.log("no tools=" + said());
"##]
    .concat();
    let Some(stdout) = run_app_js(&script) else {
        return;
    };
    let lines: Vec<&str> = stdout.lines().collect();
    let opened = [
        "opened=Yep | [▾ 12 steps true]",
        " | tool:step 1 | tool:step 2 | tool:step 3 | tool:step 4 | tool:step 5 | tool:step 6",
        " | Halfway.",
        " | tool:step 7 | tool:step 8 | tool:step 9 | tool:step 10 | tool:step 11 | tool:step 12",
        " | I filed the issue. | Thanks | Anytime.",
    ]
    .concat();
    assert_eq!(
        lines,
        [
            "folded=Yep | [▸ 12 steps false] | I filed the issue. | Thanks | Anytime.",
            opened.as_str(),
            "focused=true",
            "kept=true",
            "folded again=Yep | [▸ 12 steps false] | I filed the issue. | Thanks | Anytime.",
            "working=Go | [▸ 2 steps · Bash: cargo test false]",
            "left=[▾ 5 steps · Bash: step 5 true] | tool:step 1 | tool:step 2 | tool:step 3 | tool:step 4 | tool:step 5",
            "slid=[▾ 5 steps · Bash: step 6 true] | tool:step 2 | tool:step 3 | tool:step 4 | tool:step 5 | tool:step 6",
            "next=[▾ 5 steps true] | tool:step 2 | tool:step 3 | tool:step 4 | tool:step 5 | tool:step 6 | Next | [▸ 1 step · Bash: next false]",
            "sent=[▾ 5 steps true] | tool:step 3 | tool:step 4 | tool:step 5 | tool:step 6 | tool:step 7 | Next | [▸ 1 step false] | Then",
            "newer=[▸ 2 steps · Bash: after false]",
            "background=Go | [▸ 1 step false] | Watching CI.",
            "no tools=Hi | Hello.",
        ],
        "{stdout}"
    );
}

/// A turn that ends by running `switchboard respawn` (queue-and-clean's
/// last step, assets/phone/app.js's `turns`) keeps the text before that call
/// open after its folded steps, however many respawns follow it (a landing
/// that retried its respawn); a turn a finished background task woke
/// starts at a dim line, with no avatar. Skipped without Node.
#[test]
fn a_respawned_turns_last_line_stays_open_and_a_wake_up_is_a_dim_line() {
    let script = [CHANNEL_PAGE, r##"
vm.runInContext(`
  state.me = { paired: true, computer: "mac" };
  listen();
  state.open = "5";
`, ctx);
const update = (state) => source.onmessage({ data: JSON.stringify({ live: true, tabs: [{ pane: "%5", index: 2, name: "phone", state, turn: { prompt: "go" } }] }) });
const draw = (messages) => vm.runInContext("state.messages = " + JSON.stringify(messages) + "; render(true);", ctx);
const said = () => app.querySelectorAll("div.msg").map((m) => {
  const row = m.querySelector("button.steps");
  if (row) return "[" + row.textContent + "]";
  const tool = m.querySelector("div.tool");
  if (tool) return "tool:" + tool.textContent;
  const task = m.querySelector("div.task");
  if (task) return "dim:" + task.textContent + (m.querySelector(".avatar") ? " avatar" : "");
  return (m.querySelector(".avatar") ? "@" : "") + m.querySelector("div.body").textContent.trim();
}).join(" | ");
const landed = [
  { who: "you", text: "/queue-and-clean", at: 1 },
  { who: "tool", text: "Bash: switchboard landing", at: 2 },
  { who: "claude", text: "Landing.", at: 3 },
  { who: "tool", text: "Bash: gh pr merge", at: 4 },
  { who: "claude", text: "queue-and-clean: success (merged)", at: 5 },
  { who: "tool", text: "Bash: switchboard respawn -c '/repo' claude --resume abc", at: 6 },
];
for (const state of ["idle", "working"]) {
  update(state);
  draw(landed);
  console.log(state + "=" + said());
}
app.querySelector("button.steps").click();
console.log("opened=" + said());
app.querySelector("button.steps").click();
draw([...landed, { who: "tool", text: "Bash: switchboard respawn -c '/repo' claude --resume abc", at: 7 }]);
console.log("retried=" + said());
update("idle");
draw([
  { who: "you", text: "Watch CI", at: 10 },
  { who: "tool", text: "Monitor: CI", at: 11 },
  { who: "claude", text: "Watching.", at: 12 },
  { who: "task", text: "a background task finished", at: 20 },
  { who: "tool", text: "Bash: gh run view", at: 21 },
  { who: "claude", text: "CI passed.", at: 22 },
]);
console.log("woken=" + said());
"##]
    .concat();
    let Some(stdout) = run_app_js(&script) else {
        return;
    };
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "idle=@/queue-and-clean | [▸ 3 steps] | @queue-and-clean: success (merged)",
            "working=@/queue-and-clean | [▸ 3 steps] | @queue-and-clean: success (merged)",
            "opened=@/queue-and-clean | [▾ 3 steps] | tool:Bash: switchboard landing | @Landing. | tool:Bash: gh pr merge | tool:Bash: switchboard respawn -c '/repo' claude --resume abc | @queue-and-clean: success (merged)",
            "retried=@/queue-and-clean | [▸ 4 steps] | @queue-and-clean: success (merged)",
            "woken=@Watch CI | [▸ 1 step] | @Watching. | dim:a background task finished | [▸ 1 step] | @CI passed.",
        ],
        "{stdout}"
    );
}

/// Swiping right in a tab's channel (assets/phone/app.js's `swipeable`), run
/// in Node over the page: a touch moving right past a third of the screen's
/// width, or flicked, opens the drawer and clears the address's tab; a short
/// one, or a quick one held still before letting go, or one a second finger
/// joins, on the channel or the drawer, slides back, leaving the tab open; one moving mostly down, or
/// starting in the composer's field or its tray of images, changes nothing,
/// while one starting on a reply's code block moves the channel; and a
/// redraw asked for mid-swipe is drawn once as it ends, leaving only the
/// screen it ended on; skipped without Node.
#[test]
fn swiping_right_in_a_channel_opens_the_drawer() {
    let script = [CHANNEL_PAGE, r##"
const tab = (wait) => ({ pane: "%5", index: 2, name: "phone", state: "working", wait, turn: { prompt: "go" } });
const update = (t) => source.onmessage({ data: JSON.stringify({ live: true, tabs: [t] }) });
let drawn = 0;
const channel = ctx.channel;
ctx.channel = (stick) => { drawn++; channel(stick); };
const open = () => vm.runInContext(`state.open = "5"; render();`, ctx);
vm.runInContext(`state.me = { paired: true, computer: "mac" }; listen();`, ctx);
update(tab("Edit app.js"));
open();
const screens = () => app.children.map((c) => (c.className.includes("drawer-screen") ? "drawer" : "channel")).join(",");
const touch = (type, x, y, at, on) => {
  const e = new Event(type, { touches: type === "touchend" ? [] : [{ clientX: x, clientY: y }], timeStamp: at });
  (on || app.children[app.children.length - 1]).dispatchEvent(e);
  return e.defaultPrevented;
};
// A path of touches from (x, y), each move `step` milliseconds apart, then let go.
const swipe = (points, step, on) => {
  touch("touchstart", points[0][0], points[0][1], 0, on);
  const prevented = points.slice(1).map(([x, y], i) => touch("touchmove", x, y, (i + 1) * step, on));
  return prevented;
};
const slid = () => new Promise((done) => setTimeout(done, 300));
(async () => {
  const screen = app.children[0];
  console.log("move not passive=" + (screen.options.touchmove.passive === false));
  console.log("down=" + swipe([[100, 300], [104, 320], [110, 380]], 100) + " " + screens());
  touch("touchend", 0, 0, 400);
  const field = app.querySelector(".composer textarea");
  console.log("field=" + swipe([[20, 600], [80, 600], [300, 600]], 100, field) + " " + screens());
  touch("touchend", 0, 0, 400, field);
  vm.runInContext(`state.chosen = [{ file: "a.png", url: "blob:a" }]; render();`, ctx);
  const shot = app.querySelector(".tray img");
  console.log("tray=" + swipe([[20, 500], [80, 500], [300, 500]], 100, shot) + " " + screens());
  touch("touchend", 0, 0, 400, shot);
  vm.runInContext(`state.chosen = []; render();`, ctx);
  // A reply's code block, which wraps rather than scrolling, so a swipe
  // starting on it moves the channel, sliding back once let go short.
  const pre = new Node("pre");
  app.querySelector(".messages").append(pre);
  console.log("code=" + swipe([[20, 200], [40, 204], [100, 206]], 200, pre) + " " + screens());
  touch("touchend", 0, 0, 800, pre);
  await slid();
  console.log("code back=" + vm.runInContext("state.open", ctx) + " " + screens());
  pre.remove();
  // A short swipe, with the stream and another caller asking for redraws.
  drawn = 0;
  console.log("short=" + swipe([[20, 300], [40, 304], [100, 306]], 200) + " " + screens() + " " + app.children[1].style.transform);
  update(tab("Bash: cargo test"));
  vm.runInContext("render(true)", ctx);
  console.log("held=" + drawn + " " + app.querySelector(".doing").textContent);
  touch("touchend", 0, 0, 400);
  await slid();
  console.log("back=" + vm.runInContext("state.open", ctx) + " " + screens() + " drawn " + drawn + " " + app.querySelector(".doing").textContent + " " + JSON.stringify(app.children[0].style.transform));
  // Quick, then held still before letting go.
  swipe([[20, 300], [40, 300], [90, 300]], 10);
  touch("touchend", 0, 0, 600);
  await slid();
  console.log("paused=" + vm.runInContext("state.open", ctx) + " " + screens());
  // A second finger mid-swipe.
  swipe([[20, 300], [40, 300], [90, 300]], 100);
  touch("touchstart", 90, 300, 300);
  await slid();
  console.log("second finger=" + vm.runInContext("state.open", ctx) + " " + screens() + " " + JSON.stringify(app.children[0].style.transform));
  // A second finger on the drawer it uncovered, then the first let go far.
  swipe([[20, 300], [40, 300], [90, 300]], 100);
  touch("touchstart", 30, 300, 300, app.children[0].children[0]);
  touch("touchmove", 300, 300, 400);
  touch("touchend", 0, 0, 500);
  await slid();
  console.log("on the drawer=" + vm.runInContext("state.open", ctx) + " " + screens() + " buzzes=" + buzzes);
  // Past a third of the width.
  addresses.length = 0;
  // Past it, back short of it and past it again: one buzz, as it first crosses.
  swipe([[20, 300], [60, 300], [200, 310], [100, 310], [200, 310]], 100);
  touch("touchend", 0, 0, 600);
  await slid();
  console.log("far=" + vm.runInContext("state.open", ctx) + " " + screens() + " " + addresses.join("") + " buzzes=" + buzzes);
  // A flick: short, but quick.
  open();
  swipe([[20, 300], [40, 300], [90, 300]], 10);
  touch("touchend", 0, 0, 30);
  await slid();
  console.log("flick=" + vm.runInContext("state.open", ctx) + " " + screens());
})();
"##]
    .concat();
    let Some(stdout) = run_app_js(&script) else {
        return;
    };
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "move not passive=true",
            "down=false,false channel",
            "field=false,false channel",
            "tray=false,false channel",
            "code=true,true drawer,channel",
            "code back=5 channel",
            "short=true,true drawer,channel translateX(80px)",
            "held=0 ⠹ Edit app.js",
            r#"back=5 channel drawn 1 ⠹ Bash: cargo test """#,
            "paused=5 channel",
            r#"second finger=5 channel """#,
            "on the drawer=5 channel buzzes=0",
            "far=null drawer # buzzes=1",
            "flick=null drawer",
        ],
        "{stdout}"
    );
}

/// An image shown whole (assets/phone/app.js's `full`), run in Node over a
/// page of its own: a pinch zooms it about the fingers up to 5×, a drag pans
/// it with its edges held at the screen's, a double tap toggles 2.5× and
/// fitted, a tap closes it only while fitted, an update keeps its zoom, and
/// its × closes it; skipped without Node.
#[test]
fn an_image_shown_whole_pinches_pans_and_double_taps() {
    let script = [CHANNEL_PAGE, r##"
ctx.window.innerHeight = 800;
const tab = { pane: "%5", index: 2, name: "phone", state: "working", wait: "", turn: { prompt: "go" } };
const update = () => source.onmessage({ data: JSON.stringify({ live: true, tabs: [tab] }) });
vm.runInContext(`state.me = { paired: true, computer: "mac" }; listen();`, ctx);
update();
let at = 0;
const show = () => {
  vm.runInContext(`state.open = "5"; state.full = "a.png"; render();`, ctx);
  // Fitted: the whole width, centered under the notch's padding.
  app.querySelector(".full img").getBoundingClientRect = () => ({ left: 0, top: 200, width: 390, height: 400 });
};
const img = () => app.querySelector(".full img");
const shown = () => (img() ? JSON.stringify(img().style.transform || "fitted") : "closed");
const send = (type, init) => { at += 20; img().dispatchEvent(new Event(type, { timeStamp: at, ...init })); };
const ptr = (type, id, x, y) => send(type, { pointerId: id, clientX: x, clientY: y });
const tap = (x, y) => { ptr("pointerdown", 9, x, y); ptr("pointerup", 9, x, y); send("click", { clientX: x, clientY: y }); };
const wait = (ms) => new Promise((done) => setTimeout(done, ms));
(async () => {
  show();
  // Two fingers 100px apart, spread to 300px, their middle moved 100px right.
  ptr("pointerdown", 1, 100, 300);
  ptr("pointerdown", 2, 200, 300);
  ptr("pointermove", 2, 400, 300);
  console.log("pinched=" + shown());
  ptr("pointermove", 2, 2000, 300);
  console.log("most=" + shown().includes("scale(5)"));
  ptr("pointermove", 2, 150, 300);
  console.log("least=" + shown());
  ptr("pointermove", 2, 400, 300);
  ptr("pointerup", 2, 400, 300);
  ptr("pointerup", 1, 100, 300);
  send("click", { clientX: 250, clientY: 300 });
  tap(250, 300);
  await wait(400);
  console.log("tapped zoomed=" + shown());
  // Dragged far left, held at the right edge.
  ptr("pointerdown", 1, 100, 100);
  ptr("pointermove", 1, -900, 100);
  ptr("pointerup", 1, -900, 100);
  console.log("panned=" + shown());
  update();
  console.log("updated=" + shown());
  tap(100, 300);
  tap(100, 300);
  console.log("double=" + shown());
  tap(100, 300);
  tap(100, 300);
  console.log("again=" + shown() + " settle=" + img().classList.has("settle"));
  app.querySelector(".full button.close").click();
  console.log("x=" + shown());
  show();
  console.log("reopened=" + shown());
  tap(100, 300);
  console.log("before=" + shown());
  await wait(400);
  console.log("tapped fitted=" + shown());
})();
"##]
    .concat();
    let Some(stdout) = run_app_js(&script) else {
        return;
    };
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            r#"pinched="translate(-200px, -200px) scale(3)""#,
            "most=true",
            r#"least="fitted""#,
            r#"tapped zoomed="translate(-200px, -200px) scale(3)""#,
            r#"panned="translate(-780px, -200px) scale(3)""#,
            r#"updated="translate(-780px, -200px) scale(3)""#,
            r#"double="fitted""#,
            r#"again="translate(-150px, -200px) scale(2.5)" settle=true"#,
            "x=closed",
            r#"reopened="fitted""#,
            r#"before="fitted""#,
            "tapped fitted=closed",
        ],
        "{stdout}"
    );
}

/// The drawer ends on the app's build, short, and says which of the page
/// and its worker is behind the server's build; every request names both.
#[test]
fn the_drawer_ends_on_the_build_and_says_when_the_worker_is_behind() {
    let script = r#"
const vm = require("vm");
const fs = require("fs");
const source = fs.readFileSync(process.argv[1], "utf8").replace('const BUILD = "dev";', 'const BUILD = "1a2b3c4d5e6f";');
const ctx = vm.createContext({
  Node: class Node {},
  document: { getElementById: () => ({ querySelector: () => null }), addEventListener: () => {}, createElement: (tag) => ({ tag, children: [], className: "", setAttribute() {}, append(...k) { this.children.push(...k); } }), createTextNode: (text) => ({ text }) },
  window: { addEventListener: () => {} },
  fetch: () => new Promise(() => {}),
  console,
});
vm.runInContext(source, ctx);
const line = ([worker, server]) => vm.runInContext(`
  workerBuild = ${JSON.stringify(worker)};
  state.me = { build: ${JSON.stringify(server)} };
  ((l) => l.className + "=" + l.children.map((c) => c.text).join("") + "|" + buildHeader()["X-Switchboard-Build"])(buildLine());
`, ctx);
// [the worker's build, the server's]; a server older than the page names none.
for (const c of [[null, "1a2b3c4d5e6f"], ["none", "1a2b3c4d5e6f"], ["1a2b3c4d5e6f", "1a2b3c4d5e6f"], ["9f8e7d6c5b4a", "1a2b3c4d5e6f"], ["unknown", "1a2b3c4d5e6f"], ["9f8e7d6c5b4a", "9f8e7d6c5b4a"], ["1a2b3c4d5e6f", "9f8e7d6c5b4a"], ["9f8e7d6c5b4a", undefined]]) console.log(line(c));
// A page left open across a restart reads the server's build again.
ctx.fetch = async () => ({ status: 200, ok: true, json: async () => ({ paired: true, build: "9f8e7d6c5b4a" }) });
vm.runInContext("readServerBuild()", ctx).then(() => console.log("server=" + vm.runInContext("state.me.build", ctx)));
"#;
    let Some(out) = run_app_js(script) else {
        return;
    };
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [
            "build-line=build 1a2b3c4|page=1a2b3c4d5e6f",
            "build-line=build 1a2b3c4|page=1a2b3c4d5e6f worker=none",
            "build-line=build 1a2b3c4|page=1a2b3c4d5e6f worker=1a2b3c4d5e6f",
            "build-line behind=worker behind, close and reopen the app|page=1a2b3c4d5e6f worker=9f8e7d6c5b4a",
            "build-line behind=worker behind, close and reopen the app|page=1a2b3c4d5e6f worker=unknown",
            "build-line behind=app behind, close and reopen the app|page=1a2b3c4d5e6f worker=9f8e7d6c5b4a",
            "build-line behind=worker behind, close and reopen the app|page=1a2b3c4d5e6f worker=1a2b3c4d5e6f",
            "build-line behind=worker behind, close and reopen the app|page=1a2b3c4d5e6f worker=9f8e7d6c5b4a",
            "server=9f8e7d6c5b4a",
        ],
        "{out}"
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
    { index: 2, name: "stable note IDs", repo: "switchboard", branch: "stable-note-ids", state: "waiting", needs: true, wait: "Bash: git push", stage: "needs", group: "Needs you", words: "waiting on you · Bash: git push" },
    { index: 3, name: "M-d uncommitted", repo: "switchboard", branch: "diff-uncommitted", state: "working", wait: "Edit diff.rs", stage: "working", group: "Working", words: "working · Edit diff.rs" },
    { index: 8, name: "handoff", repo: "notes-sync", branch: "main", state: "idle", stage: "idle", group: "Idle", words: "idle for 2h" },
  ];
  JSON.stringify(groups(wordsOf(${JSON.stringify(q)})).map((g) => [g.name, g.tabs.map((t) => t.index)]));
`, ctx);
for (const q of ["", "HANDOFF", "3", "diff-UNCOMMITTED", "note sync", "deploy"]) console.log(q + "=" + run(q));
// A server older than the page sends no groups: its tabs by their state.
console.log("old=" + vm.runInContext(`
  state.tabs = [
    { index: 2, name: "a", state: "waiting", needs: true },
    { index: 8, name: "b", state: "idle" },
    { index: 3, name: "c", state: "background", wait: "Edit diff.rs" },
  ];
  JSON.stringify(groups([]).map((g) => [g.name, g.tabs.map((t) => t.index)])) + " " + markOf(state.tabs[2])[1] + " " + sayingOf(state.tabs[2]);
`, ctx));
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
            r#"old=[["Needs you",[2]],["Working",[3]],["Idle",[8]]] working spin Edit diff.rs"#,
        ],
        "{stdout}"
    );
}

/// The drawer's groups (assets/phone/app.js's `groups` and `foldGroup`), run in
/// Node over a page made of plain objects: each tab under the group the
/// server sent with it, in the sidebar's order, a tab waiting on a shell
/// under Monitoring and a queued one under In merge queue with its place; a
/// heading folding and unfolding its group, the fold kept across a reload,
/// and a search opening a folded group while it has words; skipped without
/// Node.
#[test]
fn the_drawers_groups_fold_and_a_search_opens_them() {
    let script = PRELUDE.to_string()
        + r#"
const store = { data: {}, getItem(k) { return k in this.data ? this.data[k] : null; }, setItem(k, v) { this.data[k] = String(v); } };
ctx.localStorage = store;
const base = Object.assign({}, ctx);
const TABS = [
  { index: 1, pane: "%1", name: "asks", state: "waiting", needs: true, stage: "needs", group: "Needs you", group_tone: "needs", mark: "●", tone: "needs", words: "waiting on you" },
  { index: 2, pane: "%2", name: "paging", state: "working", stage: "working", group: "Working", group_tone: "working", mark: "⠋", spins: true, tone: "working", words: "working 2m" },
  { index: 3, pane: "%3", name: "load test", state: "background", stage: "monitoring", group: "Monitoring", group_tone: "working", mark: "⠋", spins: true, tone: "working", words: "waiting on 1 shell", turn: { prompt: "(a background task finished)" } },
  { index: 4, pane: "%4", name: "webhooks", state: "idle", stage: "queued", group: "In merge queue", group_tone: "landing", mark: "▲", tone: "landing", words: "in merge queue at 2" },
  { index: 5, pane: "%5", name: "handoff", state: "idle", stage: "idle", group: "Idle", group_tone: "idle", mark: "○", tone: "idle", words: "idle for 2h" },
];
const load = (c) => {
  app.replaceChildren();
  vm.runInContext(fs.readFileSync(process.argv[1], "utf8"), c);
  vm.runInContext("state.me = { paired: true, computer: 'studio' }; state.live = true; state.tabs = " + JSON.stringify(TABS) + "; render();", c);
};
load(ctx);
const heads = () => app.all((x) => x.attrs["data-stage"]).map((x) => x.text + (x.attrs["aria-expanded"] === "false" ? "*" : "") + (x.tag === "button" ? "" : "!")).join(",");
const head = (stage) => app.all((x) => x.attrs["data-stage"] === stage)[0];
const rows = () => app.all((x) => x.attrs["data-pane"]).map((x) => x.text).join(" | ");
const out = [];
out.push("heads=" + heads());
out.push("rows=" + rows());
head("working").fire("click");
out.push("folded=" + heads() + " rows=" + app.all((x) => x.attrs["data-pane"]).map((x) => x.attrs["data-pane"]).join());
head("working").fire("click");
out.push("open=" + heads());
head("idle").fire("click");
out.push("kept=" + store.data["drawer-folds"]);
// Reloaded: the page's script run again over the same storage.
const again = vm.createContext(Object.assign({}, base));
load(again);
out.push("reloaded=" + heads());
vm.runInContext("state.search = 'handoff'; drawer();", again);
out.push("search=" + heads() + " rows=" + rows());
// A tap on a heading during a search changes no fold.
head("idle").fire("click");
vm.runInContext("state.search = ''; drawer();", again);
out.push("cleared=" + heads());
// A tab starting to wait shows under Needs you, open.
vm.runInContext("state.tabs.unshift({ index: 6, pane: '%6', name: 'new', state: 'waiting', needs: true, stage: 'needs', group: 'Needs you', mark: '●', tone: 'needs', words: 'waiting on you' }); drawer();", again);
out.push("waits=" + heads() + " rows=" + app.all((x) => x.attrs["data-pane"]).map((x) => x.attrs["data-pane"]).join());
console.log(out.join("\n"));
"#;
    let Some(stdout) = run_app_js(&script) else {
        return;
    };
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "heads=▾ Needs you1,▾ Working1,▾ Monitoring1,▾ In merge queue1,▾ Idle1",
            "rows=●1askswaiting on you | ⠹2pagingworking 2m | ⠹3load testwaiting on 1 shell | ▲4webhooksin merge queue at 2 | ○5handoffidle for 2h",
            "folded=▾ Needs you1,▸ Working1*,▾ Monitoring1,▾ In merge queue1,▾ Idle1 rows=1,3,4,5",
            "open=▾ Needs you1,▾ Working1,▾ Monitoring1,▾ In merge queue1,▾ Idle1",
            r#"kept=["idle"]"#,
            "reloaded=▾ Needs you1,▾ Working1,▾ Monitoring1,▾ In merge queue1,▸ Idle1*",
            "search=▾ Idle1! rows=○5handoffidle for 2h",
            "cleared=▾ Needs you1,▾ Working1,▾ Monitoring1,▾ In merge queue1,▸ Idle1*",
            "waits=▾ Needs you2,▾ Working1,▾ Monitoring1,▾ In merge queue1,▸ Idle1* rows=6,1,2,3,4",
        ],
        "{stdout}"
    );
}

/// Claude's replies (assets/phone/app.js's `markdown`), rendered in Node
/// with the vendored markdown-it over a stand-in document and read back as
/// HTML: a table in a box of its own with its header row, a link by its
/// text opening in the browser, a code block, raw HTML and a `javascript:`
/// link left as text, and a message of yours left as typed; skipped without
/// Node.
#[test]
fn claudes_replies_are_markdown_and_yours_stay_text() {
    let script = r##"
const vm = require("vm");
const fs = require("fs");
const path = require("path");
class Node {
  constructor(tag) { this.tagName = tag; this.children = []; this.attrs = {}; this.className = ""; this.text = ""; this.listeners = {};
    const node = this;
    this.classList = { add: (c) => { node.className = (node.className + " " + c).trim(); }, remove() {} };
  }
  get textContent() { return this.text + this.children.map((c) => c.textContent).join(""); }
  setAttribute(k, v) { this.attrs[k] = String(v); }
  addEventListener(k, f) { this.listeners[k] = f; }
  append(...kids) { this.children.push(...kids); }
  all() { return this.children.flatMap((c) => [c, ...c.all()]); }
}
// An element as HTML, its text escaped, to read what was built.
const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const html = (n) => n.tagName === "#text" ? esc(n.text)
  : "<" + n.tagName + (n.className ? ' class="' + n.className + '"' : "")
    + Object.entries(n.attrs).map(([k, v]) => " " + k + '="' + v + '"').join("") + ">"
    + n.children.map(html).join("") + "</" + n.tagName + ">";
const ctx = vm.createContext({
  Node,
  document: { getElementById: () => null, addEventListener: () => {}, createElement: (t) => new Node(t), createTextNode: (text) => Object.assign(new Node("#text"), { text }) },
  window: { addEventListener: () => {} },
  fetch: () => new Promise(() => {}),
  console,
});
vm.runInContext(fs.readFileSync(path.join(path.dirname(process.argv[1]), "markdown-it.min.js"), "utf8"), ctx);
vm.runInContext(fs.readFileSync(process.argv[1], "utf8"), ctx);
const body = (who, text) => {
  ctx.m = { who, text, at: 1 };
  const msg = vm.runInContext("message(m, 0, [m])", ctx);
  return msg.children.find((c) => c.className.split(" ").includes("body"));
};
const cases = [
  ["claude", "| Tab | State |\n| --- | :-: |\n| 12 | **waiting** |"],
  ["claude", "See [#12](https://github.com/o/r/issues/12)."],
  ["claude", "```rust\nfn main() {}\n```"],
  ["claude", "<img src=x onerror=alert(1)>"],
  ["claude", "[run](javascript:alert(1)) and [mail](mailto:a@b.c)"],
  ["claude", "Updated README.md; see https://example.com"],
  ["you", "make it **bold** <b>"],
];
for (const [who, text] of cases) console.log(html(body(who, text)));
// Folded by the text shown: a long address alone doesn't fold a reply.
console.log("long link folded=" + body("claude", "[x](https://example.com/" + "a".repeat(800) + ")").className.includes("folded"));
console.log("long text folded=" + body("claude", "word ".repeat(150)).className.includes("folded"));
// Folded by the lines shown: a table's separator and the blank lines
// between blocks aren't lines, a code block's lines are.
console.log("short table folded=" + body("claude", "| a |\n| - |\n" + "| x |\n".repeat(9)).className.includes("folded"));
console.log("loose list folded=" + body("claude", "- x\n\n".repeat(6)).className.includes("folded"));
console.log("long code folded=" + body("claude", "```\n" + "x\n".repeat(12) + "```").className.includes("folded"));
"##;
    let Some(stdout) = run_app_js(script) else {
        return;
    };
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            r#"<div class="body md"><div class="table"><table><thead><tr><th>Tab</th><th class="center">State</th></tr></thead><tbody><tr><td>12</td><td class="center"><strong>waiting</strong></td></tr></tbody></table></div></div>"#,
            r#"<div class="body md"><p>See <a href="https://github.com/o/r/issues/12" target="_blank" rel="noopener noreferrer">#12</a>.</p></div>"#,
            r#"<div class="body md"><pre><code>fn main() {}</code></pre></div>"#,
            r#"<div class="body md"><p>&lt;img src=x onerror=alert(1)&gt;</p></div>"#,
            r#"<div class="body md"><p>[run](javascript:alert(1)) and <span class="link">mail</span></p></div>"#,
            r#"<div class="body md"><p>Updated README.md; see <a href="https://example.com" target="_blank" rel="noopener noreferrer">https://example.com</a></p></div>"#,
            r#"<div class="body">make it **bold** &lt;b&gt;</div>"#,
            "long link folded=false",
            "long text folded=true",
            "short table folded=false",
            "loose list folded=false",
            "long code folded=true",
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

/// The composer's bar (assets/phone/app.css's `.composer`) gives its text box
/// 8 pt above and below, the home-indicator inset overlapping the space below
/// rather than adding to it (the channel's screen leaving the inset to it), and
/// 8 pt alone while the keyboard is open, where the screen leaves out the inset.
#[test]
fn the_composer_spaces_its_text_box_evenly() {
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
    for (selector, declaration) in [
        (".composer", "padding: var(--s2);"),
        (
            ".composer",
            "padding-bottom: max(var(--s2), var(--inset-bottom) - var(--s2));",
        ),
        (".keyboard .composer", "padding-bottom: var(--s2);"),
        (".screen.channel-screen", "padding-bottom: 0;"),
    ] {
        assert!(
            rule(selector).contains(declaration),
            "{selector}: {}",
            rule(selector)
        );
    }
    assert!(
        rule(".keyboard .screen").contains("padding-bottom: 0;"),
        "{}",
        rule(".keyboard .screen")
    );
}

/// The keyboard's class (assets/phone/app.js's `followKeyboard`), run in
/// Node over a stand-in window: set while the visual viewport is shorter than
/// the window, with the page's top and height following the viewport as it
/// scrolls, and cleared as it grows back; skipped without Node.
#[test]
fn the_keyboard_open_is_told_from_the_visual_viewport() {
    let script = r#"
const vm = require("vm");
const fs = require("fs");
const classes = new Set();
const viewport = { height: 800, offsetTop: 0, listeners: {}, addEventListener(k, f) { (this.listeners[k] ||= []).push(f); } };
const vars = new Map();
const ctx = vm.createContext({
  document: {
    getElementById: () => null,
    addEventListener: () => {},
    documentElement: {
      classList: { toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)) },
      style: { setProperty: (k, v) => vars.set(k, v), removeProperty: (k) => vars.delete(k) },
    },
  },
  window: { addEventListener: () => {}, innerHeight: 800, visualViewport: viewport },
  fetch: () => new Promise(() => {}),
  console,
});
vm.runInContext(fs.readFileSync(process.argv[1], "utf8"), ctx);
const fire = (kind) => { for (const f of viewport.listeners[kind] || []) f(); };
const resize = (height) => { viewport.height = height; fire("resize"); };
const app = () => [...vars].map(([k, v]) => k + ":" + v).join(" ");
console.log("closed=" + classes.has("keyboard") + " " + app());
resize(470.5);
console.log("open=" + classes.has("keyboard") + " " + app());
viewport.offsetTop = 120;
fire("scroll");
console.log("scrolled " + app());
viewport.offsetTop = 0;
resize(800);
console.log("closed again=" + classes.has("keyboard") + " " + app());
"#;
    let Some(stdout) = run_app_js(script) else {
        return;
    };
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "closed=false ",
            "open=true --viewport-top:0px --viewport-height:470.5px",
            "scrolled --viewport-top:120px --viewport-height:470.5px",
            "closed again=false ",
        ],
        "{stdout}"
    );
}

/// A home-screen app cut short above the home indicator (assets/phone/app.js's
/// `followKeyboard`): marked clipped while the window is shorter than the
/// screen's upright side, and not once it reaches it, run in Node over a
/// stand-in window; skipped without Node.
#[test]
fn a_home_screen_app_cut_short_is_marked_clipped() {
    let script = r#"
const vm = require("vm");
const fs = require("fs");
const classes = new Set();
const vars = new Map();
const viewport = { height: 873, offsetTop: 0, addEventListener() {} };
const win = { addEventListener: () => {}, innerWidth: 430, innerHeight: 873, visualViewport: viewport };
const ctx = vm.createContext({
  document: {
    getElementById: () => null,
    addEventListener: () => {},
    documentElement: {
      classList: { toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)) },
      style: { setProperty: (k, v) => vars.set(k, v), removeProperty: (k) => vars.delete(k) },
    },
  },
  navigator: { standalone: true },
  screen: { width: 430, height: 932 },
  window: win,
  fetch: () => new Promise(() => {}),
  console,
});
vm.runInContext(fs.readFileSync(process.argv[1], "utf8"), ctx);
console.log("short=" + classes.has("clipped"));
win.innerHeight = 932;
viewport.height = 932;
vm.runInContext("followKeyboard()", ctx);
console.log("whole=" + classes.has("clipped"));
"#;
    let Some(stdout) = run_app_js(script) else {
        return;
    };
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, ["short=true", "whole=false"], "{stdout}");
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

/// `remind`, which the phone's route asks: a typed or offered time sets
/// the tab's reminder, which the sidebar and the phone's tabs show with
/// the time left and its note, and `-c` clears it; a time it can't read
/// is refused in the reminder box's words, as are a pane not running
/// Claude and Settings' "Reminders" off; each set and clear is logged.
#[test]
fn the_phone_sets_and_clears_a_claude_tabs_reminder() {
    let server = TestServer::start();
    let mut client = server.attach(120, 36);
    client.wait_text("$");
    server.ok(&["new-tab", "-d", "-n", "plain"]);
    let plain = server.list()["sessions"][0]["tabs"][1]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (code, _, err) = server.run(&["remind", "-t", &plain, "-s", "phone", "45m"]);
    assert_eq!(code, 1, "a pane not running Claude");
    assert!(err.contains("isn't running Claude"), "{err}");

    let pane = claude_pane(&server);
    let mut stream = watch(&server);
    next_line(&mut stream, |v| {
        v["tabs"].as_array().is_some_and(|t| !t.is_empty())
    });

    let (code, _, err) = server.run(&["remind", "-t", &pane, "-s", "phone", "soonish"]);
    assert_eq!(code, 1, "an unreadable time");
    assert!(
        err.contains("\"soonish\" isn't a time I know. Try 45m"),
        "the box's words: {err}"
    );

    let said = server.ok(&[
        "remind",
        "-t",
        &pane,
        "-s",
        "phone",
        "-n",
        "check the deploy",
        "--",
        "45m",
    ]);
    assert!(said.starts_with("Reminder for tab 1 "), "{said}");
    let set = next_line(&mut stream, |v| v["tabs"][0]["remind"].is_object());
    let remind = &set["tabs"][0]["remind"];
    assert_eq!(remind["note"], "check the deploy", "{set}");
    assert_eq!(remind["due"], false, "{set}");
    let left = remind["at"].as_i64().unwrap() - switchboard::util::now_secs();
    assert!(
        (44 * 60..=45 * 60).contains(&left),
        "45 minutes out: {left}"
    );
    assert_eq!(set["reminders"], true);
    let times: Vec<&str> = set["remind_times"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["label"].as_str().unwrap())
        .collect();
    assert_eq!(times, switchboard::mission::remind::CHOICES, "{set}");
    assert!(
        set["remind_times"][0]["clock"]
            .as_str()
            .unwrap()
            .contains(':'),
        "{set}"
    );
    client.wait_text("remind in 45m");

    // One of the box's times, as the sheet sends a chosen one.
    server.ok(&["remind", "-t", &pane, "-s", "phone", "--", "In 3 hours"]);
    let later = next_line(&mut stream, |v| {
        v["tabs"][0]["remind"]["at"].as_i64().unwrap_or(0) > remind["at"].as_i64().unwrap()
    });
    assert_eq!(later["tabs"][0]["remind"]["note"], "", "no note: {later}");

    let said = server.ok(&["remind", "-t", &pane, "-s", "phone", "-c"]);
    assert!(
        said.starts_with("Cleared the reminder for tab 1 "),
        "{said}"
    );
    next_line(&mut stream, |v| v["tabs"][0]["remind"].is_null());
    let log = server.log();
    assert!(log.contains("): Reminder at "), "logged: {log}");
    assert!(log.contains("): Cleared its reminder"), "logged: {log}");

    // Settings' "Reminders" off refuses it, and the phone is told so.
    common::write_settings(&server.home, json!({ "reminders": false }));
    wait_until(&server, "the reminder refused while off", || {
        let (_, _, err) = server.run(&["remind", "-t", &pane, "-s", "phone", "45m"]);
        err.contains("reminders are off in Settings")
    });
    next_line(&mut stream, |v| v["reminders"] == false);
}

/// `stop`, which the phone's route asks: Esc pressed in a working Claude
/// pane, as Esc at the computer stops its turn, and logged; refused for an
/// idle Claude and for a pane not running Claude.
#[test]
fn the_phone_stops_a_working_claude_with_esc() {
    let server = TestServer::start();
    let mut client = server.attach(120, 36);
    client.wait_text("$");
    server.ok(&["new-tab", "-d", "-n", "plain"]);
    let plain = server.list()["sessions"][0]["tabs"][1]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (code, _, err) = server.run(&["stop", "-t", &plain, "-s", "phone"]);
    assert_eq!(code, 1, "a pane not running Claude");
    assert!(err.contains("isn't running Claude"), "{err}");

    let pane = claude_pane(&server);
    // A numbered list in Claude's own words, which isn't a prompt to answer.
    server.ok(&[
        "send-keys",
        "-t",
        &pane,
        "-l",
        "clear; printf '1. Plan it\\n2. Build it\\n'; cat -v",
    ]);
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    let (code, _, err) = server.run(&["stop", "-t", &pane, "-s", "phone"]);
    assert_eq!(code, 1, "an idle Claude");
    assert!(err.contains("its Claude isn't working"), "{err}");

    server.ok(&["set", "-t", &pane, "@agent_state", "working"]);
    let mut stream = watch(&server);
    next_line(&mut stream, |v| v["tabs"][0]["state"] == "working");
    let said = server.ok(&["stop", "-t", &pane, "-s", "phone"]);
    assert!(said.starts_with("Stopped tab 1 "), "{said}");
    wait_until(&server, "Esc pressed in the pane", || {
        screen(&server, &pane).contains("^[")
    });
    let log = server.log();
    assert!(log.contains("from your phone: tab 1 "), "logged: {log}");
    assert!(log.contains("): Stopped"), "logged: {log}");

    // Still working with a permission prompt on screen, as auto mode asks
    // one, Esc would refuse it rather than stop the turn: refused.
    // Ctrl-C can flush what is typed right after it, so the shell is asked
    // until it answers before the prompt is shown.
    server.ok(&["send-keys", "-t", &pane, "C-c"]);
    wait_until(&server, "the shell back after Ctrl-C", || {
        server.ok(&["send-keys", "-t", &pane, "-l", "echo back-$((40+2))"]);
        server.ok(&["send-keys", "-t", &pane, "Enter"]);
        std::thread::sleep(Duration::from_millis(200));
        screen(&server, &pane).contains("back-42")
    });
    show_prompt(&server, &pane);
    let (code, _, err) = server.run(&["stop", "-t", &pane, "-s", "phone"]);
    assert_eq!(code, 1, "a Claude asking");
    assert!(err.contains("asking something"), "{err}");
}

/// `pin`, which the phone's route asks: it pins a Claude tab as M-f does,
/// which the sidebar shows at the top of Needs you and the phone's tabs
/// say, `-u` unpins it, and a tab pinned with M-f reads as pinned too; a
/// pane not running Claude is refused; each pin and unpin is logged.
#[test]
fn the_phone_pins_and_unpins_a_claude_tab() {
    let server = TestServer::start();
    let mut client = server.attach(120, 36);
    client.wait_text("$");
    server.ok(&["new-tab", "-d", "-n", "plain"]);
    let plain = server.list()["sessions"][0]["tabs"][1]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (code, _, err) = server.run(&["pin", "-t", &plain, "-s", "phone"]);
    assert_eq!(code, 1, "a pane not running Claude");
    assert!(err.contains("isn't running Claude"), "{err}");

    let pane = claude_pane(&server);
    let tab = server.list()["sessions"][0]["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut stream = watch(&server);
    next_line(&mut stream, |v| v["tabs"][0]["pinned"] == false);

    let said = server.ok(&["pin", "-t", &pane, "-s", "phone"]);
    assert!(said.starts_with("Pinned tab 1 "), "{said}");
    let pinned = next_line(&mut stream, |v| v["tabs"][0]["pinned"] == true);
    assert_eq!(pinned["tabs"][0]["needs"], true, "{pinned}");
    assert_eq!(
        server.ok(&["show", "-t", &tab, "@pinned"]).trim(),
        "1",
        "pinned as M-f pins it"
    );
    client.wait_text("⚑");

    let said = server.ok(&["pin", "-t", &pane, "-s", "phone", "-u"]);
    assert!(said.starts_with("Unpinned tab 1 "), "{said}");
    next_line(&mut stream, |v| v["tabs"][0]["pinned"] == false);
    assert_eq!(server.ok(&["show", "-t", &tab, "@pinned"]).trim(), "");

    let log = server.log();
    assert!(log.contains("): Pinned it"), "logged: {log}");
    assert!(log.contains("): Unpinned it"), "logged: {log}");

    // Pinned in the workspace, the phone reads it as pinned.
    server.ok(&["set", "-t", &tab, "@pinned", "1"]);
    next_line(&mut stream, |v| v["tabs"][0]["pinned"] == true);
}

/// A page made of plain objects for assets/phone/app.js to run on in Node:
/// its `app` and `body`, the events fired on them, its timers and what it
/// posted, before the app is loaded.
const PRELUDE: &str = r#"
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
let buzzes = 0;
const ctx = vm.createContext({
  Node,
  navigator: { vibrate: () => buzzes++ },
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
"#;

/// The drawer's menu (assets/phone/app.js), run in Node over a page made
/// of plain objects: a long press on a row opens its tab's menu rather
/// than the tab, Close tab asks first, only Close closes it, and a tap
/// still opens the tab; skipped without Node.
#[test]
fn a_long_press_opens_a_tabs_menu_and_close_tab_asks_first() {
    let app = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/phone/app.js");
    let script = PRELUDE.to_string()
        + r#"vm.runInContext(fs.readFileSync(process.argv[1], "utf8"), ctx);
vm.runInContext(`
  state.me = { paired: true, computer: "studio" };
  state.tabs = [
    { index: 4, pane: "%4", name: "full titles", state: "done", needs: true, stage: "needs", group: "Needs you", group_tone: "needs", mark: "✓", tone: "passed", words: "finished" },
    { index: 3, pane: "%3", name: "M-d uncommitted", state: "working", stage: "working", group: "Working", group_tone: "working", spins: true, tone: "working", words: "working" },
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
  out.push("menu=" + options().join(",") + " open=" + vm.runInContext("state.open", ctx) + " buzzes=" + buzzes);
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
  out.push("working=" + options().join(",") + " buzzes=" + buzzes);
  button("×").fire("click");
  row("3").fire("pointerdown");
  row("3").fire("pointerup");
  row("3").fire("click");
  out.push("tap=" + vm.runInContext("state.open", ctx));
  // A redraw while a finger is down: lifting it still lets the hold go.
  vm.runInContext("state.open = null; state.tabs.push({ index: 5, pane: \"%5\", name: \"five\", state: \"idle\", stage: \"idle\", group: \"Idle\", mark: \"○\", tone: \"idle\" }); render();", ctx);
  row("5").fire("pointerdown");
  vm.runInContext("render()", ctx);
  row("5").fire("pointerup");
  timers.splice(0).forEach((f) => f && f());
  out.push("redrawn=" + vm.runInContext("state.menu", ctx));
  // Android's long press: the context menu while the hold's timer runs buzzes, once.
  row("5").fire("pointerdown");
  row("5").fire("contextmenu");
  timers.splice(0).forEach((f) => f && f());
  out.push("android=" + vm.runInContext("state.menu.pane", ctx) + " buzzes=" + buzzes);
  button("×").fire("click");
  // A tap on the lit row, outside the menu, closes it.
  row("5").fire("contextmenu");
  row("5").fire("pointerdown");
  row("5").fire("pointerup");
  row("5").fire("click");
  out.push("lit row=" + vm.runInContext("state.menu", ctx) + " open=" + vm.runInContext("state.open", ctx));
  // Close tabs from the phone and Reminders off: a tab still working offers Pin it alone.
  vm.runInContext("state.close = false; state.reminders = false", ctx);
  row("3").fire("contextmenu");
  out.push("off=" + options().join(","));
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
            "menu=Mark seen,Pin it,Remind me,Close tab open=null buzzes=1",
            "asks=true posted=0",
            "answers=No,Close*",
            "no=0 menu=null",
            "closed=/api/tabs/4/close tabs=3",
            "working=Pin it,Remind me,Close tab buzzes=1",
            "tap=3",
            "redrawn=null",
            "android=5 buzzes=2",
            "lit row=null open=null",
            "off=Pin it",
        ],
        "{stdout}"
    );
}

/// Remind me in the drawer's menu (assets/phone/app.js, board 176), run in
/// Node over a page made of plain objects: offered on every tab, with Clear
/// the reminder before it on a tab with one, whose row shows ⏰ and the time
/// left; its sheet lists the box's times with their clock times, sends a
/// chosen or typed time with the note, keeps open with the computer's words
/// for a time it can't read, and puts Set at the right; skipped without
/// Node.
#[test]
fn remind_me_in_a_tabs_menu_sets_and_clears_its_reminder() {
    let app = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/phone/app.js");
    let script = r#"
${PRELUDE}
ctx.fetch = (path, opts) => {
  if (opts && opts.method === "POST") {
    const body = JSON.parse(opts.body);
    posted.push(path + " " + opts.body);
    if (body.when === "soonish") return Promise.resolve({ ok: false, status: 409, json: async () => ({ error: "\"soonish\" isn't a time I know. Try 45m, 2h, 3pm, 15:30, nov 20 or 2027-01-04 3pm." }) });
    return Promise.resolve({ ok: true, status: 200, json: async () => ({ said: "Reminder for tab 4 full titles at Sun 10:41 AM" }) });
  }
  return new Promise(() => {});
};
vm.runInContext(fs.readFileSync(process.argv[1], "utf8"), ctx);
vm.runInContext(`
  state.me = { paired: true, computer: "studio" };
  state.remindTimes = [
    { label: "In 20 minutes", clock: "10:01" }, { label: "In 1 hour", clock: "10:41" }, { label: "In 3 hours", clock: "12:41" },
    { label: "Tomorrow at 9:00", clock: "Sun 9:00" }, { label: "Next Monday at 9:00", clock: "Mon 9:00" },
  ];
  state.tabs = [
    { index: 4, pane: "%4", name: "full titles", state: "done", needs: true, stage: "needs", group: "Needs you", mark: "✓", tone: "passed", words: "finished" },
    { index: 3, pane: "%3", name: "M-d uncommitted", state: "working", wait: "Edit diffview.lua", stage: "working", group: "Working", spins: true, tone: "working", words: "working · Edit diffview.lua", remind: { at: Math.floor(Date.now() / 1000) + 80 * 60, note: "check the deploy", due: false } },
  ];
  render();
`, ctx);
const row = (n) => app.all((x) => x.attrs["data-pane"] === n)[0];
const options = () => body.all((x) => x.attrs.role === "menuitem").map((x) => x.text);
const button = (label) => body.all((x) => x.tag === "button" && x.text === label)[0];
const field = (label) => body.all((x) => x.tag === "input" && x.attrs["aria-label"] === label)[0];
const sheet = () => body.all((x) => x.className === "box sheet")[0];
const tick = () => new Promise((r) => setImmediate(r));
const out = [];
(async () => {
  out.push("row=" + row("3").text);
  row("4").fire("contextmenu");
  out.push("none=" + options().join(","));
  button("Remind me").fire("click");
  out.push("sheet=" + sheet().attrs["aria-label"]);
  out.push("times=" + body.all((x) => x.attrs.role === "radio").map((x) => x.text).join(","));
  out.push("answers=" + body.all((x) => x.className === "answers")[0].kids.map((k) => k.text + (k.className.includes("lit") ? "*" : "")).join(","));
  field("Or type a time").value = "soonish";
  button("Set").fire("click");
  await tick();
  out.push("refused=" + body.all((x) => x.className === "refused")[0].text + " open=" + !!sheet());
  body.all((x) => x.attrs.role === "radio" && x.text.startsWith("In 1 hour"))[0].fire("click");
  field("What for").value = "check the deploy";
  button("Set").fire("click");
  await tick();
  out.push("set=" + posted[posted.length - 1] + " open=" + !!sheet());
  row("3").fire("contextmenu");
  out.push("one=" + options().join(","));
  button("Remind me").fire("click");
  out.push("change=" + sheet().attrs["aria-label"] + " note=" + field("What for").value);
  button("Cancel").fire("click");
  row("3").fire("contextmenu");
  button("Clear the reminder").fire("click");
  await tick();
  out.push("cleared=" + posted[posted.length - 1]);
  console.log(out.join("\n"));
})();
"#
    .replace("${PRELUDE}", PRELUDE);
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
            "row=⠹3M-d uncommittedworking · Edit diffview.lua⏰ in 1h 20m · check the deploy",
            "none=Mark seen,Pin it,Remind me,Close tab",
            "sheet=Remind me about tab 4 full titles",
            "times=In 20 minutes10:01,In 1 hour10:41,In 3 hours12:41,Tomorrow at 9:00Sun 9:00,Next Monday at 9:00Mon 9:00",
            "answers=Cancel,Set*",
            "refused=\"soonish\" isn't a time I know. Try 45m, 2h, 3pm, 15:30, nov 20 or 2027-01-04 3pm. open=true",
            r#"set=/api/tabs/4/remind {"when":"In 1 hour","note":"check the deploy"} open=false"#,
            "one=Pin it,Clear the reminder,Remind me,Close tab",
            "change=Change the reminder for tab 3 M-d uncommitted note=check the deploy",
            r#"cleared=/api/tabs/3/remind {"clear":true}"#,
        ],
        "{stdout}"
    );
}

/// Pin it in the drawer's menu (assets/phone/app.js), run in Node over a
/// page made of plain objects: Pin it on a tab not pinned, Unpin it on one
/// that is, each posting whether to pin without asking first; a pinned tab
/// leads Needs you with the flag in place of its mark, beside a reminder
/// come due in the server's order, as the sidebar ranks them; skipped
/// without Node.
#[test]
fn pin_it_in_a_tabs_menu_pins_and_unpins_it() {
    let app = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/phone/app.js");
    let script = r#"
${PRELUDE}
ctx.fetch = (path, opts) => {
  if (opts && opts.method === "POST") {
    posted.push(path + " " + opts.body);
    return Promise.resolve({ ok: true, status: 200, json: async () => ({ said: "Pinned tab 3" }) });
  }
  return new Promise(() => {});
};
vm.runInContext(fs.readFileSync(process.argv[1], "utf8"), ctx);
vm.runInContext(`
  state.me = { paired: true, computer: "studio" };
  state.tabs = [
    { index: 3, pane: "%3", name: "M-d uncommitted", state: "idle", needs: true, pinned: true, stage: "needs", group: "Needs you", mark: "⚑", tone: "needs", words: "idle for 2h" },
    { index: 6, pane: "%6", name: "deploy", state: "waiting", needs: true, pinned: false, remind: { at: 1, note: "", due: true }, stage: "needs", group: "Needs you", mark: "◷", tone: "needs", words: "waiting on you" },
    { index: 4, pane: "%4", name: "full titles", state: "waiting", needs: true, pinned: false, stage: "needs", group: "Needs you", mark: "●", tone: "needs", words: "waiting on you" },
    { index: 5, pane: "%5", name: "five", state: "working", needs: false, pinned: false, stage: "working", group: "Working", spins: true, tone: "working", words: "working" },
  ];
  render();
`, ctx);
const row = (n) => app.all((x) => x.attrs["data-pane"] === n)[0];
const options = () => body.all((x) => x.attrs.role === "menuitem").map((x) => x.text);
const button = (label) => body.all((x) => x.tag === "button" && x.text === label)[0];
const tick = () => new Promise((r) => setImmediate(r));
const out = [];
(async () => {
  out.push("groups=" + JSON.stringify(vm.runInContext("groups([]).map((g) => [g.name, g.tabs.map((t) => t.index)])", ctx)));
  out.push("rows=" + row("3").text + " | " + row("4").text);
  row("3").fire("contextmenu");
  out.push("pinned=" + options().join(","));
  button("Unpin it").fire("click");
  await tick();
  out.push("unpin=" + posted[posted.length - 1] + " menu=" + vm.runInContext("state.menu", ctx));
  row("5").fire("contextmenu");
  out.push("not=" + options().join(","));
  button("Pin it").fire("click");
  await tick();
  out.push("pin=" + posted[posted.length - 1] + " menu=" + vm.runInContext("state.menu", ctx));
  console.log(out.join("\n"));
})();
"#
    .replace("${PRELUDE}", PRELUDE);
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
            r#"groups=[["Needs you",[3,6,4]],["Working",[5]]]"#,
            "rows=⚑3M-d uncommittedidle for 2h | ●4full titleswaiting on you",
            "pinned=Unpin it,Remind me,Close tab",
            r#"unpin=/api/tabs/3/pin {"pin":false} menu=null"#,
            "not=Pin it,Remind me,Close tab",
            r#"pin=/api/tabs/5/pin {"pin":true} menu=null"#,
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
        // A page whose name was pointed at 127.0.0.1 is refused.
        let mut stream = std::net::TcpStream::connect(at).unwrap();
        write!(
            stream,
            "GET /api/tabs HTTP/1.1\r\nHost: rebound.example:{}\r\nConnection: close\r\n\r\n",
            at.port()
        )
        .unwrap();
        let mut refused = String::new();
        std::io::Read::read_to_string(&mut stream, &mut refused).unwrap();
        assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");
        assert_eq!(json_of(at, "/api/me")["paired"], true);
        let tabs = json_of(at, "/api/tabs");
        assert_eq!(tabs["live"], true);
        assert_eq!(tabs["tabs"].as_array().unwrap().len(), 21);
        let turns = json_of(at, "/api/tabs/13/turns");
        assert!(!turns["messages"].as_array().unwrap().is_empty());
    }

    /// A long made-up conversation's channel opens on its last 20 turns,
    /// and `?before=` answers the turns before, back to its start.
    #[test]
    fn the_preview_answers_a_long_channels_turns_before() {
        let at = preview(&assets());
        let asks = |turns: &Value| -> Vec<String> {
            turns["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|m| m["who"] == "you")
                .map(|m| m["text"].as_str().unwrap().to_string())
                .collect()
        };
        let last = json_of(at, "/api/tabs/31/turns");
        assert_eq!(asks(&last).len(), 20, "{last}");
        assert!(asks(&last)[19].starts_with("Step 64:"), "{last}");
        assert_eq!(last["earlier"], true);
        let from = last["from"].as_u64().unwrap();
        let before = json_of(at, &format!("/api/tabs/31/turns?before={from}"));
        assert_eq!(asks(&before).len(), 20);
        assert!(asks(&before)[0].starts_with("Step 25:"), "{before}");
        assert!(asks(&before)[19].starts_with("Step 44:"), "{before}");
        assert!(
            before["messages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|m| m["offset"].as_u64().unwrap() < from)
        );
        let mut from = before["from"].as_u64().unwrap();
        let mut earliest = before;
        while earliest["earlier"] == true {
            earliest = json_of(at, &format!("/api/tabs/31/turns?before={from}"));
            from = earliest["from"].as_u64().unwrap();
        }
        assert_eq!(from, 0);
        assert!(asks(&earliest)[0].starts_with("Step 1:"), "{earliest}");
    }

    /// The preview stamps app.js with its own build, as the phone server
    /// stamps app.js and sw.js.
    #[test]
    fn the_preview_stamps_the_app_with_its_builds_commit() {
        let at = preview(&assets());
        let (status, app) = http(at, "GET", "/app.js", None);
        assert_eq!(status, 200);
        let stamp = format!("const BUILD = \"{}", env!("SWITCHBOARD_COMMIT"));
        assert!(app.contains(&stamp), "{}", &app[..app.len().min(600)]);
        assert!(!app.contains("const BUILD = \"dev\";"));
        // The build the drawer tells a page behind by is the same one.
        let me = json_of(at, "/api/me");
        assert!(
            stamp.ends_with(me["build"].as_str().unwrap().split('+').next().unwrap()),
            "{me}"
        );
    }

    /// Stop on the made-up working tab leaves it idle, and is refused on a
    /// tab that isn't working, as the server refuses it.
    #[test]
    fn stop_idles_the_made_up_working_tab_and_refuses_an_idle_one() {
        let at = preview(&assets());
        let state = || {
            json_of(at, "/api/tabs")["tabs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["pane"] == "%16")
                .unwrap()["state"]
                .clone()
        };
        assert_eq!(state(), "working");
        let (status, body) = http(at, "POST", "/api/tabs/16/stop", Some(json!({})));
        assert_eq!(status, 200, "{body}");
        assert_eq!(state(), "idle");
        let (status, body) = http(at, "POST", "/api/tabs/16/stop", Some(json!({})));
        assert_eq!(status, 409, "{body}");
        assert!(body.contains("isn't working"), "{body}");
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
    fn a_reminder_is_set_refused_and_cleared_on_the_made_up_tabs() {
        let at = preview(&assets());
        let tabs = json_of(at, "/api/tabs");
        assert_eq!(tabs["reminders"], true);
        assert_eq!(tabs["remind_times"].as_array().unwrap().len(), 5);
        let pane = tabs["tabs"][0]["pane"]
            .as_str()
            .unwrap()
            .trim_start_matches('%')
            .to_string();
        let path = format!("/api/tabs/{pane}/remind");
        let (status, body) = http(
            at,
            "POST",
            &path,
            Some(json!({"when": "soonish", "note": ""})),
        );
        assert_eq!(status, 409);
        assert!(body.contains("isn't a time I know"), "{body}");
        let (status, body) = http(
            at,
            "POST",
            &path,
            Some(json!({"when": "45m", "note": "check the deploy"})),
        );
        assert_eq!(status, 200, "{body}");
        let set = json_of(at, "/api/tabs")["tabs"][0]["remind"].clone();
        assert_eq!(
            (set["note"].as_str(), set["due"].as_bool()),
            (Some("check the deploy"), Some(false)),
            "{set}"
        );
        let (status, _) = http(at, "POST", &path, Some(json!({"clear": true})));
        assert_eq!(status, 200);
        assert!(json_of(at, "/api/tabs")["tabs"][0]["remind"].is_null());
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

/// The Claude pane's conversation given a transcript, as Claude Code
/// writes one, and `watch` followed until its tab names it.
fn with_transcript(server: &TestServer) -> UnixStream {
    let dir = server.home.join(".claude/projects/phone");
    std::fs::create_dir_all(&dir).unwrap();
    let line = json!({"type": "user", "message": {"content": "show me"}});
    std::fs::write(dir.join("s-phone.jsonl"), format!("{line}\n")).unwrap();
    let mut stream = watch(server);
    next_line(&mut stream, |v| v["tabs"][0]["transcript"].is_string());
    stream
}

/// Waits a moment for what would come of a `look` that should add nothing.
fn grace() {
    std::thread::sleep(Duration::from_millis(1500) * common::patience());
}

/// `look` in a Claude tab (src/phone/looks.rs): its pictures copied into
/// the phone's images folder and listed on the tab's Claude pane in what
/// `watch` sends, a before and after marked so, and a video adding none.
#[test]
fn a_look_in_a_claude_tab_reaches_its_channel_on_the_phone() {
    let server = TestServer::start_with_settings(r#"{"phone": true}"#);
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let pane = claude_pane(&server);
    let mut stream = with_transcript(&server);
    let picture = |name: &str, size: u32| {
        let file = server.dir.path().join(name);
        common::four_colours(&file, size);
        file.to_string_lossy().into_owned()
    };
    let (a, b) = (picture("a.png", 64), picture("b.png", 32));
    server.ok(&["look", "-t", &pane, &a, &b]);
    let looked = next_line(&mut stream, |v| {
        v["tabs"][0]["looks"]
            .as_array()
            .is_some_and(|l| l.len() == 1)
    });
    let look = &looked["tabs"][0]["looks"][0];
    assert_eq!(look["before_after"], false);
    let images = server.state.join("phone/images");
    let names: Vec<&str> = look["images"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 2, "{look}");
    for name in &names {
        assert!(images.join(name).is_file(), "{name} copied");
    }

    server.ok(&["look", "-t", &pane, "--before-after", &a, &b]);
    let both = next_line(&mut stream, |v| {
        v["tabs"][0]["looks"]
            .as_array()
            .is_some_and(|l| l.len() == 2)
    });
    assert_eq!(both["tabs"][0]["looks"][1]["before_after"], true);

    let video = server.dir.path().join("demo.mp4");
    std::fs::write(&video, b"\0\0\0\x18ftypmp42").unwrap();
    server.ok(&["look", "-t", &pane, video.to_str().unwrap()]);
    grace();
    server.ok(&["set", "-t", &pane, "@agent_state", "waiting"]);
    let after = next_line(&mut stream, |v| v["tabs"][0]["state"] == "waiting");
    assert_eq!(after["tabs"][0]["looks"].as_array().unwrap().len(), 2);
}

/// With Settings' `phone_look_pictures` off, `look` copies nothing for the
/// phone and its tab lists no look.
#[test]
fn a_look_with_the_setting_off_reaches_nothing_on_the_phone() {
    let server =
        TestServer::start_with_settings(r#"{"phone": true, "phone_look_pictures": false}"#);
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let pane = claude_pane(&server);
    let mut stream = with_transcript(&server);
    let file = server.dir.path().join("a.png");
    common::four_colours(&file, 64);
    server.ok(&["look", "-t", &pane, file.to_str().unwrap()]);
    grace();
    server.ok(&["set", "-t", &pane, "@agent_state", "waiting"]);
    let after = next_line(&mut stream, |v| v["tabs"][0]["state"] == "waiting");
    assert_eq!(after["tabs"][0]["looks"], json!([]));
    assert!(!server.state.join("phone/images").exists());
}

/// A look's message in the channel (assets/phone/app.js's `message`): its
/// thumbnails titled Before and After, still in sight with the turn's steps
/// folded, and a tap on one showing it whole; skipped without Node.
#[test]
fn a_looks_pictures_show_in_the_channel_and_whole_on_a_tap() {
    let script = [CHANNEL_PAGE, r##"
vm.runInContext(`
  state.me = { paired: true, computer: "mac" };
  listen();
  state.open = "5";
`, ctx);
source.onmessage({ data: JSON.stringify({ live: true, tabs: [{ pane: "%5", index: 2, name: "phone", state: "done", turn: { prompt: "go" } }] }) });
const messages = [
  { who: "you", text: "Show me", at: 1 },
  { who: "tool", text: "Bash: switchboard look --before-after a.png b.png", at: 2 },
  { who: "look", text: "Showed a before and after", images: ["a.png", "b.png"], titles: ["Before", "After"], at: 3 },
  { who: "tool", text: "Bash: cargo test", at: 4 },
  { who: "claude", text: "Here they are.", at: 5 },
];
vm.runInContext("state.messages = " + JSON.stringify(messages) + "; render(true);", ctx);
const shots = app.querySelectorAll("button.shot");
console.log("shots=" + shots.length);
console.log("titles=" + app.querySelectorAll("figcaption").map((c) => c.textContent).join(","));
shots[1].click();
console.log("whole=" + vm.runInContext("state.full", ctx));
"##]
    .concat();
    let Some(stdout) = run_app_js(&script) else {
        return;
    };
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        ["shots=2", "titles=Before,After", "whole=b.png"],
        "{stdout}"
    );
}

/// Settings' "Buzz the phone" (`phone_haptics`) reaches the phone with the
/// drawer's snapshot, as `haptics`, on by default and false once turned off.
#[test]
fn the_phone_reads_whether_to_buzz_with_the_tabs() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    claude_pane(&server);
    let mut stream = watch(&server);
    let first = next_line(&mut stream, |v| v["tabs"].as_array().is_some());
    assert_eq!(first["haptics"], true, "{first}");
    common::write_settings(&server.home, json!({ "phone_haptics": false }));
    next_line(&mut stream, |v| v["haptics"] == false);
}

/// assets/phone/app.js's `buzz`, in Node over a stand-in for the page:
/// `navigator.vibrate` where the browser has it, else a click on a hidden
/// switch's label made for each buzz and removed after it, and neither with
/// Settings' `phone_haptics` off. Skipped without Node.
#[test]
fn buzz_vibrates_or_clicks_the_hidden_switch_unless_turned_off() {
    let script = r#"
const vm = require("vm");
const fs = require("fs");
const added = [];
class Node {
  constructor() { this.kids = []; }
  append(...k) { for (const kid of k) kid.parent = this; this.kids.push(...k); if (this === body) added.push(...k); }
}
class Element extends Node {
  constructor(tag) { super(); this.tag = tag; this.attrs = {}; this.style = {}; this.clicks = 0; this.clickedIn = false; }
  setAttribute(k, v) { this.attrs[k] = v; }
  addEventListener() {}
  click() { this.clicks++; this.clickedIn = this.parent === body; }
  remove() { const kids = this.parent.kids; kids.splice(kids.indexOf(this), 1); this.parent = null; }
}
const body = new Element("body");
const vibrated = [];
const navigator = {};
const ctx = vm.createContext({
  Node,
  navigator,
  document: {
    body,
    getElementById: () => null,
    addEventListener: () => {},
    createElement: (tag) => new Element(tag),
    createTextNode: (t) => Object.assign(new Node(), { text: t }),
  },
  window: { addEventListener: () => {} },
  history: { replaceState: () => {} },
  location: { hash: "" },
  fetch: () => new Promise(() => {}),
  console,
});
vm.runInContext(fs.readFileSync(process.argv[1], "utf8"), ctx);
const run = (code) => vm.runInContext(code, ctx);
// What was vibrated, what stays in the page, and each label added: clicked
// once while in the page, hidden, holding a switch Tab never reaches.
const show = (l) => [l.tag, l.clicks, l.clickedIn, l.style.display, "style" in l.attrs, l.attrs["aria-hidden"], l.kids[0].attrs.type, "switch" in l.kids[0].attrs, l.kids[0].attrs.tabindex].join(":");
const seen = () => console.log(vibrated.join(",") + " " + body.kids.length + " " + (added.length ? added.map(show).join(" ") : "-"));

navigator.vibrate = (ms) => vibrated.push(ms);
run("buzz()");
seen();
run("state.haptics = false; buzz()");
seen();
delete navigator.vibrate;
run("state.haptics = true; buzz(); buzz()");
seen();
run("state.haptics = false; buzz()");
seen();
// Answering: a tick of a question that takes several answers buzzes nothing, its Submit does.
navigator.vibrate = (ms) => vibrated.push(ms);
run(`state.haptics = true;
  state.open = "4";
  state.tabs = [{ index: 4, pane: "%4", name: "t", state: "waiting", prompt: { id: "q", multi: true, options: [{ n: 1 }, { n: 2 }] } }];`);
vibrated.length = 0;
run(`answer("1"); state.sent = null; answer("submit")`);
console.log("answered=" + vibrated.length);
"#;
    let Some(out) = run_app_js(script) else {
        return;
    };
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines,
        [
            "10 0 -",
            "10 0 -",
            "10 0 label:1:true:none:false:true:checkbox:true:-1 label:1:true:none:false:true:checkbox:true:-1",
            "10 0 label:1:true:none:false:true:checkbox:true:-1 label:1:true:none:false:true:checkbox:true:-1",
            "answered=1",
        ],
        "{out}"
    );
}

/// Mission control's panel (assets/phone/app.js, boards 185 to 187), run in
/// Node over a page made of plain objects: a channel's ⋯ leads with
/// Mission control, which opens the panel on its views with what needs you
/// counted; a view's items show under its groups, an item's menu lists
/// what the computer sent, an option that asks does so with No at the left
/// and Yes lit at the right before it posts, Go to tab opens that tab's
/// channel, and a menu whose options changed is closed; skipped without
/// Node.
#[test]
fn the_mission_control_panel_lists_views_and_runs_their_options() {
    let script = PRELUDE.to_string()
        + r##"vm.runInContext(fs.readFileSync(process.argv[1], "utf8"), ctx);
vm.runInContext(`
  state.me = { paired: true, computer: "studio" };
  state.tabs = [
    { index: 2, pane: "%12", name: "stable note IDs", state: "idle", stage: "idle", group: "Idle", mark: "○", tone: "idle", words: "idle" },
  ];
  const take = { label: "Take it out of the merge queue", shown: "Take it out of the merge queue…", ask: "Take #93 out of the merge queue?" };
  state.views = [
    { key: "landing", title: "Landing", line: "1 in the queue", needs: 0, groups: [
      { title: "In the queue", items: [
        { id: "q93", title: "#93 stable note IDs", detail: "tab 2 · next train", mark: "▲", tone: "landing",
          options: [{ label: "Go to tab 2", shown: "Go to tab 2", go: "%12" }, take] },
      ] },
    ], acts: [] },
    { key: "ci", title: "CI", line: "1 failing", needs: 1, mark: "✕", tone: "failed", groups: [], acts: [], empty: "Nothing is running." },
  ];
  state.open = "12";
  render();
`, ctx);
const text = (n) => n.text;
const options = () => body.all((x) => x.attrs.role === "menuitem").map(text);
const button = (label) => body.all((x) => x.tag === "button" && x.text === label)[0];
const out = [];
(async () => {
  app.all((x) => x.attrs["aria-label"] === "Tab 2's menu")[0].fire("click");
  out.push("menu=" + options().join(","));
  button("Mission control›").fire("click");
  const rows = () => body.all((x) => x.attrs["data-view"]).map((r) => r.attrs["data-view"] + ":" + r.text);
  out.push("views=" + rows().join("|"));
  body.all((x) => x.attrs["data-view"] === "landing")[0].fire("click");
  out.push("items=" + body.all((x) => x.attrs["data-item"]).map(text).join("|"));
  body.all((x) => x.attrs["data-item"] === "q93")[0].fire("click");
  out.push("options=" + options().join(","));
  button("Take it out of the merge queue…").fire("click");
  const answers = body.all((x) => x.className === "answers")[0].kids.map((k) => k.text + (k.className.includes("lit") ? "*" : ""));
  out.push("asks=" + body.text.includes("Take #93 out of the merge queue? [Y/n]") + " answers=" + answers.join(",") + " posted=" + posted.length);
  button("Yes").fire("click");
  await new Promise((r) => setImmediate(r));
  out.push("posted=" + posted.join(","));
  // A menu whose options the computer changed is closed.
  body.all((x) => x.attrs["data-item"] === "q93")[0].fire("click");
  vm.runInContext("state.views[0].groups[0].items[0].options.pop(); staleMenu();", ctx);
  out.push("stale=" + vm.runInContext("state.menu", ctx));
  body.all((x) => x.attrs["data-item"] === "q93")[0].fire("click");
  button("Go to tab 2").fire("click");
  out.push("go=" + vm.runInContext("state.open + ' ' + state.panel", ctx));
  console.log(out.join("\n"));
})();
"##;
    let Some(stdout) = run_app_js(&script) else {
        return;
    };
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "menu=Mission control›,Pin it,Remind me,Close tab",
            "views=landing:○Landing1 in the queue|ci:✕CI11 failing",
            "items=▲#93 stable note IDstab 2 · next train",
            "options=Go to tab 2,Take it out of the merge queue…",
            "asks=true answers=No,Yes* posted=0",
            "posted=/api/views/landing/item",
            "stale=null",
            "go=12 null",
        ],
        "{stdout}"
    );
}

/// A view tapped in mission control's panel (assets/phone/app.js) opens
/// full screen over it, with no strip beside it and ‹ at its header's left,
/// as a step in the page's history; ‹ goes back through the phone's own
/// back, and the panel shows again on its list, still open; skipped without
/// Node.
#[test]
fn a_view_opens_full_screen_and_back_returns_to_the_panels_list() {
    let script = PRELUDE.to_string()
        + r##"const pushed = [];
let popstate = null;
let backs = 0;
ctx.history = {
  state: null,
  replaceState() {},
  pushState(s) { pushed.push(s); ctx.history.state = s; },
  back() { backs++; ctx.history.state = null; popstate(); },
};
ctx.window.addEventListener = (name, f) => { if (name === "popstate") popstate = f; };
vm.runInContext(fs.readFileSync(process.argv[1], "utf8"), ctx);
vm.runInContext(`
  state.me = { paired: true, computer: "studio" };
  state.tabs = [
    { index: 2, pane: "%12", name: "stable note IDs", state: "idle", stage: "idle", group: "Idle", mark: "○", tone: "idle", words: "idle" },
  ];
  state.views = [
    { key: "landing", title: "Landing", line: "1 in the queue", needs: 0, groups: [
      { title: "In the queue", items: [
        { id: "q93", title: "#93 stable note IDs", detail: "tab 2 · next train", mark: "▲", tone: "landing",
          options: [{ label: "Go to tab 2", shown: "Go to tab 2", go: "%12" }] },
      ] },
    ], acts: [] },
  ];
  state.open = "12";
  render();
  edgeSwipe();
`, ctx);
const button = (label) => body.all((x) => x.tag === "button" && x.text === label)[0];
const of = (cls) => body.all((x) => x.className.split(" ").includes(cls));
const out = [];
app.all((x) => x.attrs["aria-label"] === "Tab 2's menu")[0].fire("click");
button("Mission control›").fire("click");
timers.splice(0).forEach((f) => f && f());
body.all((x) => x.attrs["data-view"] === "landing")[0].fire("click");
const screen = of("panel-view")[0];
const head = screen.kids[0];
out.push("screen=" + screen.parent.className + " strip=" + screen.all((x) => x.className === "panel-strip").length + " head=" + head.kids.map((k) => k.text).join("|"));
out.push("items=" + screen.all((x) => x.attrs["data-item"]).length + " panel=" + of("panel")[0].all((x) => x.attrs["data-item"]).length + " pushed=" + JSON.stringify(pushed));
head.all((x) => x.attrs["aria-label"] === "Back to the views")[0].fire("click");
timers.splice(0).forEach((f) => f && f());
out.push("back=" + backs + " view=" + vm.runInContext("state.panel.view", ctx) + " screens=" + of("panel-view").length + " layers=" + of("panel-layer").length);
out.push("list=" + body.all((x) => x.attrs["data-view"]).map((r) => r.attrs["data-view"]).join("|"));
// Going to a tab from an open view takes its history step back first.
body.all((x) => x.attrs["data-view"] === "landing")[0].fire("click");
body.all((x) => x.attrs["data-item"] === "q93")[0].fire("click");
button("Go to tab 2").fire("click");
out.push("go=" + backs + " " + ctx.history.state + " " + vm.runInContext("state.open + ' ' + state.panel", ctx));
// A tab opened under an open view, as a tapped push opens one, closes it first.
app.all((x) => x.attrs["aria-label"] === "Tab 2's menu")[0].fire("click");
button("Mission control›").fire("click");
body.all((x) => x.attrs["data-view"] === "landing")[0].fire("click");
vm.runInContext("openTab('12')", ctx);
out.push("pushed=" + backs + " " + ctx.history.state + " " + vm.runInContext("state.panel", ctx));
console.log(out.join("\n"));
"##;
    let Some(stdout) = run_app_js(&script) else {
        return;
    };
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "screen=panel-layer strip=0 head=‹|Landing",
            r#"items=1 panel=0 pushed=[{"view":"landing"}]"#,
            "back=1 view=null screens=0 layers=1",
            "list=landing",
            "go=2 null 12 null",
            "pushed=3 null null",
        ],
        "{stdout}"
    );
}

/// The notification a tab's `ready` makes, as the phone's stream carries
/// it in its Notifications view, with the stream it came in.
fn ready_notice(server: &TestServer) -> (UnixStream, Value) {
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-d", "-n", "plain"]);
    let tab = server.list()["sessions"][0]["tabs"][1]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["ready", "-t", &tab, "Diff"]);
    let mut stream = watch(server);
    let notice = |v: &Value| -> Option<Value> {
        let view = v["views"]
            .as_array()?
            .iter()
            .find(|v| v["key"] == "notifications")?
            .clone();
        view["groups"]
            .as_array()?
            .iter()
            .flat_map(|g| g["items"].as_array().cloned().unwrap_or_default())
            .find(|i| i["options"].as_array().is_some_and(|o| !o.is_empty()))
    };
    let line = next_line(&mut stream, |v| notice(v).is_some());
    (stream, notice(&line).unwrap())
}

/// Mission control's views in the phone's stream (src/phone/mission.rs):
/// the Notifications view's items, each with the options of its mission
/// control menu the phone keeps, while Settings' `phone_views` is on, and
/// no views with it off; and the commands a tap sends, which run a kept
/// option and refuse one the phone leaves out or never offered.
#[test]
fn the_stream_carries_mission_controls_views_and_their_options_run() {
    let server = TestServer::start_with_settings(r#"{ "phone": true }"#);
    let (mut stream, item) = ready_notice(&server);
    let labels: Vec<&str> = item["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["label"].as_str().unwrap())
        .collect();
    assert!(labels.contains(&"Dismiss it"), "{item}");
    // Going to a tab running no Claude has no channel on the phone.
    assert!(!labels.iter().any(|l| l.starts_with("Go to tab")), "{item}");
    let id = item["id"].as_str().unwrap();

    // What mission control offers that the phone leaves out (its tab,
    // which has no channel), and what neither offers, are refused.
    let (code, _, err) = server.run(&["phone-item", "notifications", id, "Go to tab 2"]);
    assert_ne!(code, 0);
    assert!(err.contains("computer's screen"), "{err}");
    let (code, _, err) = server.run(&["phone-item", "notifications", "gone", "Dismiss it"]);
    assert_ne!(code, 0);
    assert!(err.contains("changed"), "{err}");

    // Typed in a pane, as a Claude would, it is refused: only the phone
    // app, which asks first, sends it.
    let (code, _, err) = server.run_binary(&["phone-item", "notifications", id, "Dismiss it"]);
    assert_ne!(code, 0);
    assert!(err.contains("phone app alone"), "{err}");

    // A kept option runs, and the notification is gone from the view.
    let said = server.ok(&["phone-item", "notifications", id, "Dismiss it"]);
    assert_eq!(said.trim(), "Dismissed.");
    let id = id.to_string();
    next_line(&mut stream, |v| {
        v["views"].as_array().is_some() && !v.to_string().contains(&id)
    });

    // Off, no views, and a tap is refused.
    common::write_settings(&server.home, json!({ "phone": true, "phone_views": false }));
    let off = next_line(&mut stream, |v| v["views"].is_null());
    assert!(off["tabs"].is_array());
    let (code, _, err) = server.run(&["phone-view", "ci", "Read CI again now"]);
    assert_ne!(code, 0);
    assert!(err.contains("off"), "{err}");
}

/// The Usage view in the phone's panel (src/phone/mission.rs): listed
/// last while Settings' `usage` is on, its line the fullest limit and
/// today's spend, a limit past 90% counted as needing you and drawn in the
/// attention tone, nothing on it acting; gone with `usage` off.
#[test]
fn the_panel_lists_usage_while_it_is_on() {
    let server = TestServer::start_with_settings(r#"{ "phone": true }"#);
    let usage = server.state.join("usage");
    std::fs::create_dir_all(&usage).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    std::fs::write(
        usage.join("limits"),
        json!({
            "five_hour": { "used_percentage": 92, "resets_at": now + 7_830 },
            "seven_day": { "used_percentage": 40, "resets_at": now + 3 * 86_400 },
        })
        .to_string(),
    )
    .unwrap();
    let mut stream = watch(&server);
    let usage_of = |v: &Value| -> Option<Value> {
        v["views"]
            .as_array()?
            .iter()
            .find(|v| v["key"] == "usage")
            .cloned()
    };
    let line = next_line(&mut stream, |v| {
        usage_of(v).is_some_and(|u| !u["usage"].is_null())
    });
    let views = line["views"].as_array().unwrap();
    assert_eq!(views.last().unwrap()["key"], "usage", "after the rest");
    let u = usage_of(&line).unwrap();
    assert_eq!(u["title"], "Usage");
    assert_eq!(u["line"], "5-hour 92% · $0.00 today", "{u}");
    assert_eq!(u["needs"], 1);
    assert_eq!(u["tone"], "needs");
    let five = &u["usage"]["limits"][0];
    assert_eq!(five["used"], 92);
    assert_eq!(five["tone"], "needs");
    assert!(
        five["resets"].as_str().unwrap().starts_with("resets in 2h"),
        "{five}"
    );
    assert_eq!(u["acts"], json!([]));
    let (code, _, err) = server.run(&["phone-view", "usage", "Anything"]);
    assert_ne!(code, 0);
    assert!(err.contains("Usage"), "{err}");

    common::write_settings(&server.home, json!({ "phone": true, "usage": false }));
    let off = next_line(&mut stream, |v| {
        v["views"].as_array().is_some() && usage_of(v).is_none()
    });
    assert!(
        off["views"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["key"] != "usage")
    );
}

/// The Usage view in assets/phone/app.js: each limit a meter in its tone
/// with its percent, its reset and, paced over, where it ends up; today's
/// and the week's spend; today's top tabs, a tap on an open one opening its
/// channel and a closed one doing nothing; skipped without Node.
#[test]
fn the_usage_view_draws_its_limits_spend_and_tabs() {
    let script = PRELUDE.to_string()
        + r##"vm.runInContext(fs.readFileSync(process.argv[1], "utf8"), ctx);
vm.runInContext(`
  state.me = { paired: true, computer: "studio" };
  state.tabs = [
    { index: 2, pane: "%12", name: "stable note IDs", state: "idle", stage: "idle", group: "Idle", mark: "○", tone: "idle", words: "idle" },
  ];
  state.views = [
    { key: "usage", title: "Usage", line: "5-hour 92% · $14.20 today", needs: 1, mark: "●", tone: "needs", groups: [], acts: [], loaded: true,
      usage: { today: 14.2, week: 80.5,
        limits: [
          { key: "five_hour", name: "5-hour", used: 92, resets: "resets in 2h 10m", pace: 130, over: true, tone: "needs" },
          { key: "seven_day", name: "Week", used: 30, resets: "resets Fri 9:00 AM", pace: null, over: false, tone: "passed" },
        ],
        tabs: [
          { index: 2, name: "stable note IDs", spent: 6, pane: "%12" },
          { index: 9, name: "closed one", spent: 3, pane: null },
        ] } },
  ];
  state.open = "12";
  render();
`, ctx);
const button = (label) => body.all((x) => x.tag === "button" && x.text === label)[0];
const out = [];
app.all((x) => x.attrs["aria-label"] === "Tab 2's menu")[0].fire("click");
button("Mission control›").fire("click");
out.push("row=" + body.all((x) => x.attrs["data-view"])[0].text);
body.all((x) => x.attrs["data-view"] === "usage")[0].fire("click");
for (const l of body.all((x) => x.attrs["data-limit"])) {
  const fill = l.all((x) => (x.className || "").startsWith("fill"))[0];
  out.push(l.attrs["data-limit"] + "=" + l.text + " fill=" + fill.className + " " + fill.style.width);
}
out.push("spend=" + body.all((x) => x.className === "spend").map((s) => s.text).join("|"));
const tabs = body.all((x) => (x.className || "").includes("usage-tab"));
out.push("tabs=" + tabs.map((t) => t.tag + ":" + t.text).join("|"));
tabs[0].fire("click");
out.push("go=" + vm.runInContext("state.open + ' ' + state.panel", ctx));
console.log(out.join("\n"));
"##;
    let Some(stdout) = run_app_js(&script) else {
        return;
    };
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "row=●Usage15-hour 92% · $14.20 today",
            "five_hour=5-hour92%resets in 2h 10m · at this pace, 130% by then fill=fill needs 92%",
            "seven_day=Week30%resets Fri 9:00 AM fill=fill passed 30%",
            "spend=Today$14.20|This week$80.50",
            "tabs=button:●2stable note IDs$6.00|div:○9closed one$3.00",
            "go=12 null",
        ],
        "{stdout}"
    );
}
