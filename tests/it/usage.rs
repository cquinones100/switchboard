//! How the workspace is used, on a server of the test's own: a tab that
//! closes is logged for the Reopen view with its name, directory and
//! panes, a sealed review reopened from M-o's picker with its sealed try
//! beside it, a
//! plugin named in sidebar.json is run with its options on stdin, one
//! that asks for it lists its first items in the sidebar, and a statusline
//! a managed plugin skips is said once and marked.

use crate::common;

use std::time::{Duration, Instant};

use common::TestServer;

fn wait_until(what: &str, check: impl Fn() -> bool) {
    let deadline = common::deadline(Duration::from_secs(15));
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_setting_is_changed_from_the_command_line() {
    let dir = common::TempDir::new("sb-setting");
    let file = dir.path().join("settings.json");
    let run = |args: &[&str]| {
        let out = std::process::Command::new(common::BIN)
            .arg("setting")
            .args(args)
            .env("HOME", dir.path())
            .env("SWITCHBOARD_SETTINGS", &file)
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )
    };
    std::fs::write(&file, "{\"tips\": true}\n").unwrap();
    assert_eq!(run(&["review_security", "off"]), (0, "off".into()));
    assert_eq!(run(&["review_security"]), (0, "off".into()));
    let mine: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(mine["review_security"], false);
    assert!(dir.path().join("settings.json.bak").exists());
    // What isn't a value is refused, and the file left as it was.
    assert_eq!(run(&["review_security", "maybe"]).0, 2);
    assert_eq!(run(&["no_such_setting", "on"]).0, 2);
    assert_eq!(run(&["review_security", "--reset"]), (0, "on".into()));
    let mine: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert!(mine.get("review_security").is_none(), "{mine}");
    // A file that can't be read as JSON is never written over.
    std::fs::write(&file, "not json").unwrap();
    assert_eq!(run(&["review_security", "off"]).0, 1);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "not json");
}

#[test]
fn a_closed_tab_is_logged_with_its_name_and_directory() {
    let server = TestServer::start();
    let dir = server.home.join("work");
    std::fs::create_dir_all(&dir).unwrap();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    let made = server.ok(&["new-tab", "-n", "keep me", "-c", dir.to_str().unwrap()]);
    let tab = made.split_whitespace().next().unwrap_or("").to_string();
    server.ok(&["kill-tab", "-t", &tab]);
    let log = server.state.join("closed.jsonl");
    wait_until("the closing in the log", || log.exists());
    let line: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .last()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(line["kind"], "closed");
    assert_eq!(line["tabName"], "keep me");
    assert_eq!(line["path"], dir.to_str().unwrap());
    assert_eq!(line["panes"], 1);
}

#[test]
fn a_sealed_review_reopens_from_m_o_with_its_try_beside_its_claude() {
    let server = TestServer::start();
    let bin = server.home.join("bin");
    common::stand_in::Claude::new()
        .answer("echo \"$*\" >> \"$HOME/claude-ran\"")
        .install(&bin);
    common::stand_in::install(
        &bin.join("switchboard"),
        "#!/bin/sh\necho \"$*\" >> \"$HOME/sealed-ran\"\n",
    );
    let clone = server.home.join("clone");
    let worktree = server.home.join("pr-worktree");
    std::fs::create_dir_all(&clone).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::create_dir_all(&server.state).unwrap();
    let at = jiff::Timestamp::now().as_millisecond();
    let closing = serde_json::json!({
        "at": at, "id": "@9", "index": 3, "name": "Review PR 7", "kind": "closed",
        "path": clone, "session": "s-7", "lived": 600_000, "panes": 1,
        "sealed": worktree, "sealedFetch": true,
    });
    std::fs::write(server.state.join("closed.jsonl"), format!("{closing}\n")).unwrap();
    let mut client = server.attach(120, 40);
    client.wait_text("$");
    client.send(b"\x1bo");
    client.wait_text("Review PR 7");
    client.send(b"\r");
    client.wait_text("Reopen it, resuming its Claude, with a sealed try");
    client.send(b"\r");
    wait_until("its Claude resumed", || {
        std::fs::read_to_string(server.home.join("claude-ran"))
            .is_ok_and(|t| t.contains("--resume s-7"))
    });
    if cfg!(target_os = "macos") {
        wait_until("the sealed try beside it", || {
            std::fs::read_to_string(server.home.join("sealed-ran")).is_ok_and(|t| {
                t.contains(&format!(
                    "try --sealed --fetch --detached {}",
                    worktree.display()
                ))
            })
        });
    } else {
        client.wait_text("Only the review's Claude was resumed");
    }
}

/// The tab in front, from `list`: its name and directory.
fn front_tab(server: &TestServer) -> Option<(String, String)> {
    let list = server.list();
    let tab = list["sessions"][0]["tabs"]
        .as_array()?
        .iter()
        .find(|t| t["current"] == true)?
        .clone();
    Some((
        tab["name"].as_str()?.to_string(),
        tab["path"].as_str()?.to_string(),
    ))
}

#[test]
fn alt_shift_t_reopens_the_closed_tabs_newest_first() {
    let mut server = TestServer::start();
    let first = server.home.join("first");
    let second = server.home.join("second");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    // Two tabs that lived a minute each, the second closed last, logged as
    // a closing tab logs them; the server reads the log as it starts.
    let line = |at: i64, name: &str, dir: &std::path::Path| {
        serde_json::json!({
            "at": at, "id": format!("@{at}"), "index": 2, "name": name,
            "tabName": name, "kind": "closed", "path": dir, "lived": 60_000,
        })
        .to_string()
    };
    let now = switchboard::util::now_ms();
    std::fs::create_dir_all(&server.state).unwrap();
    std::fs::write(
        server.state.join("closed.jsonl"),
        format!(
            "{}\n{}\n",
            line(now - 20_000, "first one", &first),
            line(now - 10_000, "second one", &second)
        ),
    )
    .unwrap();
    server.restart();
    let mut client = server.attach(100, 30);
    client.wait_text("$");

    server.ok(&["press", "reopen-last-tab"]);
    client.wait_for("the second closed in front", |_| {
        front_tab(&server)
            .is_some_and(|(name, path)| name == "second one" && path == second.to_str().unwrap())
    });
    server.ok(&["press", "reopen-last-tab"]);
    client.wait_for("the first closed in front", |_| {
        front_tab(&server)
            .is_some_and(|(name, path)| name == "first one" && path == first.to_str().unwrap())
    });
    server.ok(&["press", "reopen-last-tab"]);
    client.wait_text("No closed tab to reopen.");
}

#[test]
fn a_named_plugin_runs_with_its_options() {
    let server = TestServer::start();
    let config = server.home.join(".config/switchboard");
    let plugins = config.join("sidebar-plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let seen = server.home.join("seen");
    let script = plugins.join("demo");
    common::stand_in::install(
        &script,
        &format!(
            "#!/bin/sh\ncat > '{}'\necho '[{{\"id\":1,\"title\":\"one\"}}]'\n",
            seen.display()
        ),
    );
    std::fs::write(
        config.join("sidebar.json"),
        r#"{ "plugins": { "demo": { "word": "hello" }, "other": false } }"#,
    )
    .unwrap();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    wait_until("the plugin to run", || {
        std::fs::read_to_string(&seen).is_ok_and(|t| t.contains("hello"))
    });
    assert_eq!(
        std::fs::read_to_string(&seen).unwrap(),
        r#"{"word":"hello"}"#
    );
}

#[test]
fn a_plugin_that_asks_for_it_lists_its_first_items_in_the_sidebar() {
    let server = TestServer::start();
    let config = server.home.join(".config/switchboard");
    let plugins = config.join("sidebar-plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let script = plugins.join("deploys");
    let items = r#"{"title":"Deploys","sidebar":2,"items":[{"id":1,"title":"api rolling out","state":"attention"},{"id":2,"title":"web done","state":"idle"},{"id":3,"title":"jobs queued"}]}"#;
    common::stand_in::install(&script, &format!("#!/bin/sh\necho '{items}'\n"));
    std::fs::write(
        config.join("sidebar.json"),
        r#"{ "plugins": { "deploys": true } }"#,
    )
    .unwrap();
    let mut client = server.attach(140, 36);
    let screen = client.wait_text("DEPLOYS 3 ! 1");
    assert!(screen.iter().any(|l| l.contains("● api rolling out")));
    assert!(screen.iter().any(|l| l.contains("○ web done")));
    assert!(
        !screen.iter().any(|l| l.contains("jobs queued")),
        "only the first two"
    );
}

/// `switchboard <args>` with `input` on stdin, as Claude Code runs a hook
/// or the statusline in `pane`.
fn piped(server: &TestServer, args: &[&str], pane: &str, input: serde_json::Value) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(common::BIN)
        .args(args)
        .env("HOME", &server.home)
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env("SWITCHBOARD", server.socket())
        .env("SWITCHBOARD_PANE", pane)
        .env_remove("SLACK_RELAY_HELPER")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("switchboard runs");
    let _ = child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input.to_string().as_bytes());
    assert!(child.wait().expect("it ends").success());
}

#[test]
fn a_statusline_s_spend_shows_on_the_usage_board_under_its_tab() {
    use serde_json::json;
    let server = TestServer::start();
    common::write_settings(
        &server.home,
        json!({"notifications": false, "tips": false, "sidebar": false}),
    );
    let mut client = server.attach(160, 40);
    client.wait_text("$");
    let first = server.list()["sessions"][0]["tabs"][0]["panes"][0].clone();
    let pane = first["id"].as_str().unwrap().to_string();
    // Its Claude is known by its session file, as Claude Code writes one.
    let pid = first["pid"].as_u64().unwrap();
    let sessions = server.home.join(".claude/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join(format!("{pid}.json")),
        json!({"pid": pid, "entrypoint": "cli", "sessionId": "s1"}).to_string(),
    )
    .unwrap();
    let session = json!({"session_id": "s1", "cwd": server.home});
    piped(&server, &["hook", "SessionStart"], &pane, session.clone());
    piped(&server, &["hook", "UserPromptSubmit"], &pane, session);
    // Two draws of the statusline, the session's cost growing between them.
    for cost in [1.0, 2.5] {
        let payload = json!({
            "session_id": "s1",
            "model": {"display_name": "Opus"},
            "workspace": {"current_dir": server.home},
            "cost": {"total_cost_usd": cost},
        });
        piped(&server, &["statusline"], &pane, payload);
    }
    let today = {
        let now = jiff::Zoned::now();
        format!("{:04}-{:02}-{:02}", now.year(), now.month(), now.day())
    };
    let log = std::fs::read_to_string(server.state.join(format!("usage/spent-{today}.tsv")))
        .expect("the day's spend log");
    let added: Vec<&str> = log
        .lines()
        .map(|l| l.split('\t').nth(2).unwrap_or(""))
        .collect();
    assert_eq!(added, ["1.0000", "1.5000"], "{log}");
    client.type_text("\x1b ");
    client.wait_text("MISSION CONTROL");
    client.type_text("vusage\r");
    let screen = client.wait_for("the tab's spend on the board", |s| {
        s.iter().any(|l| l.contains("SPEND BY QUARTER HOUR"))
            && s.iter()
                .any(|l| l.contains(" 1 ") && l.contains("$2.50") && l.contains("100%"))
    });
    assert!(
        screen.iter().any(|l| l.contains("Today $2.50")),
        "{}",
        screen.join("\n")
    );
}

/// A hook sent over the socket from `pane`, its JSON on stdin, as
/// `switchboard hook` sends it.
fn hook_from(server: &TestServer, pane: &str, event: &str, input: serde_json::Value) {
    let msg = switchboard::mux::proto::ClientMsg::Command {
        args: vec!["hook".into(), event.into()],
        cwd: server.home.clone(),
        pane: Some(pane.into()),
        stdin: Some(input.to_string().into_bytes()),
        by_hand: false,
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    switchboard::mux::socket::run_command_at(&server.socket(), &msg, &mut out, &mut err);
}

#[test]
fn a_statusline_a_managed_plugin_skips_is_said_once_and_marked() {
    use serde_json::json;
    let server = TestServer::start();
    // Switchboard's statusline turned off: a session started now isn't
    // watched, however silent its statusline stays.
    common::write_settings(
        &server.home,
        json!({"notifications": false, "tips": false, "pane_statusline": false}),
    );
    let mut client = server.attach(160, 40);
    client.wait_text("$");
    server.ok(&["new-tab"]);
    let panes: Vec<String> = server.list()["sessions"][0]["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["panes"][0]["id"].as_str().unwrap().to_string())
        .collect();
    let (silent, unwatched) = (&panes[0], &panes[1]);
    // Its Claude is known by its session file, as Claude Code writes one,
    // so the sweep keeps its watch.
    let pid = server.list()["sessions"][0]["tabs"][0]["panes"][0]["pid"]
        .as_u64()
        .unwrap();
    let sessions = server.home.join(".claude/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join(format!("{pid}.json")),
        json!({"pid": pid, "entrypoint": "cli", "sessionId": "s1"}).to_string(),
    )
    .unwrap();
    let session = json!({"session_id": "s1", "cwd": server.home, "source": "startup"});
    hook_from(&server, unwatched, "SessionStart", session.clone());
    common::write_settings(&server.home, json!({"notifications": false, "tips": false}));
    hook_from(&server, silent, "SessionStart", session.clone());
    for pane in [silent, unwatched] {
        hook_from(&server, pane, "UserPromptSubmit", session.clone());
        hook_from(&server, pane, "Stop", session.clone());
    }
    let missing = |pane: &str| server.ok(&["show", "-t", pane, "@statusline_missing"]);
    let deadline = common::deadline(Duration::from_secs(30));
    while missing(silent).trim() != "1" {
        assert!(
            Instant::now() < deadline,
            "the silent statusline wasn't marked"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    client.wait_text("Claude's statusline is off in tab 1");
    assert_eq!(missing(unwatched).trim(), "");
    assert_eq!(server.ok(&["show", "@statusline_skipped"]).trim(), "1");
    // Its statusline drawing at last takes the mark off.
    server.typed_in(silent, &["context"]);
    assert_eq!(missing(silent).trim(), "");
    assert_eq!(server.ok(&["show", "@statusline_skipped"]).trim(), "");
}
