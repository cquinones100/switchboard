//! The sidebar as a person sees it, against a server of the test's own: on
//! the left of a window wide enough for it and the panes beside it, a tab a
//! hook marked waiting under Needs you, M-s, its keys and a click moving
//! between tabs, and the cursor onto a notification bringing the
//! Notifications window to the front (the tmux version's
//! test/tmux-sidebar.test.js, for what the sidebar draws and how it is
//! moved through).
//!
//! As in tests/agent.rs, a pane counts as running Claude when a process in
//! front on its terminal has a session file, so the tests write one for the
//! pane's shell.

use crate::common;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{TestClient, TestServer, stand_in};
use serde_json::{Value, json};

const ALT: &str = "\x1b";

/// Settings for the tests: no desktop notifications from them, and these.
fn settings(server: &TestServer, extra: Value) {
    let mut all = json!({"notifications": false, "tips": false});
    for (k, v) in extra.as_object().unwrap() {
        all[k] = v.clone();
    }
    common::write_settings(&server.home, all);
}

/// A Claude session file for `pid`, as Claude Code writes one.
fn session_file(home: &Path, pid: u64, session: Value) {
    let dir = home.join(".claude/sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let mut file = json!({"pid": pid, "entrypoint": "cli"});
    for (k, v) in session.as_object().unwrap() {
        file[k] = v.clone();
    }
    std::fs::write(dir.join(format!("{pid}.json")), file.to_string()).unwrap();
}

/// `switchboard hook <event>` in `pane`, as Claude Code runs it.
fn hook(server: &TestServer, pane: &str, event: &str, input: Value) {
    let mut child = Command::new(common::BIN)
        .args(["hook", event])
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
    assert!(child.wait().expect("the hook ends").success());
}

fn tabs(list: &Value) -> Vec<Value> {
    list["sessions"][0]["tabs"].as_array().unwrap().clone()
}

fn focused(list: &Value) -> bool {
    list["sessions"][0]["sidebar"]["focused"] == true
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

/// The first pane's width, as the server laid it out.
fn pane_cols(list: &Value) -> u64 {
    tabs(list)[0]["panes"][0]["cols"].as_u64().unwrap_or(0)
}

/// The sidebar's part of a screen line: its first `cols` columns.
fn left(line: &str, cols: usize) -> String {
    line.chars().take(cols).collect()
}

/// The screen row whose sidebar part holds `text`.
fn row_of(client: &mut TestClient, text: &str) -> usize {
    let screen = client.wait_for(&format!("{text:?} in the sidebar"), |s| {
        s.iter().any(|l| left(l, 34).contains(text))
    });
    screen
        .iter()
        .position(|l| left(l, 34).contains(text))
        .unwrap()
}

#[test]
fn the_sidebar_is_on_the_left_of_a_wide_window_and_the_panes_beside_it() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    // The session's name at the top, the shell's tab under Other.
    let screen = client.wait_for("the sidebar", |s| {
        s.iter().any(|l| left(l, 34).contains("OTHER 1"))
    });
    assert!(screen[0].starts_with("  main "), "{:?}", screen[0]);
    assert_eq!(
        screen[0].chars().nth(34),
        Some('│'),
        "the line beside the panes: {:?}",
        screen[0]
    );
    // The pane is narrower by the sidebar's 34 columns and its line.
    let list = wait_list(&server, "the pane beside the sidebar", |l| {
        pane_cols(l) == 85
    });
    assert_eq!(list["sessions"][0]["sidebar"]["columns"], 35);
    client.type_text("echo beside-it\r");
    let screen = client.wait_text("beside-it");
    let line = screen
        .iter()
        .find(|l| l.contains("$ echo beside-it"))
        .unwrap();
    let col = line
        .char_indices()
        .position(|(i, _)| line[i..].starts_with("$ echo"));
    assert_eq!(col, Some(35), "the pane starts after it: {line:?}");

    // Narrower than sidebar.json's minWindowWidth (90): none, and the pane
    // takes the whole width.
    client.resize(80, 24);
    wait_list(&server, "the pane the whole width", |l| pane_cols(l) == 80);
    client.wait_for("no sidebar", |s| !s.iter().any(|l| l.contains("OTHER 1")));
    client.resize(120, 30);
    wait_list(&server, "the sidebar back", |l| pane_cols(l) == 85);

    // Turned off in Settings, it goes at once; its own width from sidebar.json.
    settings(&server, json!({"sidebar": false}));
    wait_list(&server, "the sidebar off", |l| pane_cols(l) == 120);
    client.wait_for("no sidebar", |s| !s.iter().any(|l| l.contains("OTHER 1")));
    std::fs::write(
        server.home.join(".config/switchboard/sidebar.json"),
        r#"{"width": 40}"#,
    )
    .unwrap();
    settings(&server, json!({}));
    wait_list(&server, "a wider sidebar", |l| pane_cols(l) == 120 - 41);
}

#[test]
fn a_tab_a_hook_marked_waiting_is_under_needs_you() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "s-1"}));
    // Waiting in a tab you aren't looking at.
    server.ok(&["new-tab", "-n", "other"]);
    client.wait_text("2:other");
    hook(
        &server,
        &pane,
        "PermissionRequest",
        json!({"tool_name": "Bash", "tool_input": {"command": "git push"}}),
    );
    let screen = client.wait_for("Needs you", |s| {
        s.iter().any(|l| left(l, 34).contains("NEEDS YOU 1"))
    });
    let at = |text: &str| {
        screen
            .iter()
            .position(|l| left(l, 34).contains(text))
            .unwrap_or_else(|| panic!("{text:?} isn't in the sidebar:\n{}", screen.join("\n")))
    };
    assert!(at("NEEDS YOU 1") < at("OTHER 1"), "{}", screen.join("\n"));
    assert!(at("waiting on you") > at("NEEDS YOU 1"));
    assert!(at("permission: Bash git push") > at("NEEDS YOU 1"));
    // The count at the top, beside the session's name.
    assert!(left(&screen[0], 34).contains("! 1"), "{:?}", screen[0]);
    // Answered (the next tool runs), it is working, its badge spinning.
    hook(
        &server,
        &pane,
        "PostToolUse",
        json!({"tool_name": "Bash", "tool_input": {"command": "git push"}}),
    );
    client.wait_for("Working", |s| {
        s.iter().any(|l| left(l, 34).contains("WORKING 1"))
    });
}

#[test]
fn a_question_keeps_its_tab_under_needs_you_while_a_background_agent_returns() {
    // The log's setting from the start, so the first hook finds it read.
    let server = TestServer::start_with_settings(r#"{"notifications": false, "hook_log": true}"#);
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "s-1"}));
    server.ok(&["new-tab", "-n", "other"]);
    client.wait_text("2:other");
    // A loop's turn that starts an agent in the background, then asks.
    let agent = json!({"tool_name": "Agent", "tool_use_id": "a1", "tool_input": {"description": "Security review of branch"}});
    let ask = json!({"tool_name": "AskUserQuestion", "tool_use_id": "q1", "tool_input": {"questions": [{"question": "Push it?"}]}});
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        json!({"prompt": "/queue-and-clean"}),
    );
    hook(&server, &pane, "PreToolUse", agent.clone());
    client.wait_for("its loop's stage", |s| {
        s.iter().any(|l| left(l, 34).contains("QUEUE-AND-CLEAN 1"))
    });
    hook(&server, &pane, "PreToolUse", ask.clone());
    // The agent returns: the conversation's own hook, for another call.
    hook(&server, &pane, "PostToolUse", agent);
    let waiting = server.list();
    let record = &tabs(&waiting)[0]["panes"][0]["agent"];
    assert_eq!(record["state"], "waiting", "{record}");
    assert_eq!(record["wait"], "asks: Push it?");
    assert_eq!(record["loop"], "queue");
    let screen = client.wait_for("Needs you", |s| {
        s.iter().any(|l| left(l, 34).contains("NEEDS YOU 1"))
    });
    assert!(
        !screen
            .iter()
            .any(|l| left(l, 34).contains("QUEUE-AND-CLEAN")),
        "{}",
        screen.join("\n")
    );
    // Each hook left a line in the server's log, with the setting on.
    let log = server.log();
    assert!(
        log.contains(&format!(
            "agent hook PostToolUse Agent {pane} call=a1 agent=- kind=-"
        )),
        "{log}"
    );
    assert!(!log.contains("Push it?"), "{log}");
    // The answer: the question's own call ends, and the loop goes on.
    hook(&server, &pane, "PostToolUse", ask);
    client.wait_for("its loop's stage again", |s| {
        s.iter().any(|l| left(l, 34).contains("QUEUE-AND-CLEAN 1"))
    });
}

#[test]
fn a_turn_that_ends_over_its_watches_is_under_needs_you_until_seen_then_monitoring() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "s-1"}));
    let first = tabs(&server.list())[0]["id"].as_str().unwrap().to_string();
    server.ok(&["new-tab", "-n", "other"]);
    client.wait_text("2:other");
    // Its turn starts a Monitor watch and ends while the watch runs.
    let project = server.home.join(".claude/projects/p");
    std::fs::create_dir_all(&project).unwrap();
    let at = jiff::Timestamp::now().to_string();
    let lines = [
        json!({"type": "user", "timestamp": at, "message": {"content": "watch CI"}}),
        json!({"type": "assistant", "timestamp": at, "message": {"content": [
            {"type": "tool_use", "id": "w1", "name": "Monitor", "input": {"description": "CI on #12", "command": "gh pr checks 12"}}
        ]}}),
        json!({"type": "user", "timestamp": at, "message": {"content": [
            {"type": "tool_result", "tool_use_id": "w1", "content": "Monitor started (task m1, expires in 30m unless the source ends first)"}
        ]}}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(project.join("s-1.jsonl"), text).unwrap();
    hook(&server, &pane, "UserPromptSubmit", json!({}));
    hook(&server, &pane, "Stop", json!({}));
    let screen = client.wait_for("Needs you, its watch named", |s| {
        s.iter().any(|l| left(l, 34).contains("NEEDS YOU 1"))
            && s.iter().any(|l| left(l, 34).contains("monitoring"))
    });
    assert!(
        !screen.iter().any(|l| left(l, 34).contains("MONITORING")),
        "{}",
        screen.join("\n")
    );
    // Seen: under Monitoring.
    server.ok(&["select-tab", "-t", &first]);
    client.wait_for("Monitoring once seen", |s| {
        s.iter().any(|l| left(l, 34).contains("MONITORING 1"))
            && !s.iter().any(|l| left(l, 34).contains("NEEDS YOU"))
    });
}

#[test]
fn m_s_and_its_keys_move_between_tabs() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("2:second");
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });

    // M-s: into the sidebar, the cursor on the tab you are in.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    // k: up to the first tab, which comes to the front, the focus still in
    // the sidebar; j back down.
    client.type_text("k");
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true && focused(l)
    });
    client.type_text("j");
    wait_list(&server, "the second again", |l| {
        tabs(l)[1]["current"] == true && focused(l)
    });
    client.type_text("k");
    wait_list(&server, "the first again", |l| {
        tabs(l)[0]["current"] == true
    });
    // Enter: into its pane, where typing goes.
    client.type_text("\r");
    wait_list(&server, "the focus in the pane", |l| {
        !focused(l) && tabs(l)[0]["current"] == true
    });
    client.type_text("echo in-the-first-tab\r");
    client.wait_text("in-the-first-tab");

    // M-s twice: in and back out, on the same tab.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "back out", |l| {
        !focused(l) && tabs(l)[0]["current"] == true
    });

    // A number puts the cursor on that tab; Esc leaves it in front.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("2");
    wait_list(&server, "the second in front", |l| {
        tabs(l)[1]["current"] == true
    });
    client.type_text("\x1b");
    wait_list(&server, "out, on the second", |l| {
        !focused(l) && tabs(l)[1]["current"] == true
    });

    // a: the chosen tab answered, as mission control's menu does; a tab
    // with no Claude says so.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("a");
    client.wait_text("That tab has no Claude to answer.");
    client.type_text("x");
    client.wait_for("the box closed", |s| {
        !s.iter().any(|l| l.contains("no Claude to answer"))
    });
    client.type_text("q");
    wait_list(&server, "back out", |l| !focused(l));

    // A click on a tab's row goes there.
    server.ok(&["new-tab", "-n", "third"]);
    client.wait_text("3:third");
    let row = row_of(&mut client, "  1 ");
    client.send(format!("\x1b[<0;6;{}M\x1b[<0;6;{}m", row + 1, row + 1).as_bytes());
    wait_list(&server, "the first in front", |l| {
        tabs(l)[0]["current"] == true && !focused(l)
    });
}

#[test]
fn a_message_to_a_claude_whose_prompt_is_in_vim_s_normal_mode_presses_i_first() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    // A stand-in Claude: its prompt's vim mode line at its foot, and what
    // it is sent shown as it comes.
    server.ok(&[
        "new-tab",
        "-n",
        "vim-mode",
        "--",
        "sh",
        "-c",
        "printf '%s\\n' '>' '  -- NORMAL --'; stty -icanon -echo; exec cat",
    ]);
    client.wait_text("-- NORMAL --");
    let pid = tabs(&server.list())[1]["panes"][0]["pid"].as_u64().unwrap();
    session_file(&server.home, pid, json!({"sessionId": "s-vim"}));
    wait_list(&server, "the pane read as Claude's", |l| {
        tabs(l)[1]["panes"][0]["runs_claude"] == true
    });
    // a in the sidebar: no question showing, so a message, typed in Insert
    // mode.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("a");
    client.wait_text("No question showing");
    client.type_text("go on\r");
    client.wait_text("igo on");
}

#[test]
fn on_a_short_screen_the_header_stays_and_a_click_on_more_scrolls_half_a_screen() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 14);
    client.wait_text("$");
    for n in 2..=8 {
        server.ok(&["new-tab", "-d", "-n", &format!("t{n}")]);
    }
    // The first tab in front: the list at the top, the tabs below cut off
    // and counted on the last row, the session's line at the top.
    server.ok(&[
        "select-tab",
        "-t",
        tabs(&server.list())[0]["id"].as_str().unwrap(),
    ]);
    let screen = client.wait_for("tabs cut below", |s| {
        s.iter()
            .any(|l| left(l, 34).contains("▼") && left(l, 34).contains("more"))
    });
    assert!(left(&screen[0], 34).contains("main"), "{:?}", screen[0]);
    assert!(!screen.iter().any(|l| left(l, 34).contains("▲")));
    let down = row_of(&mut client, "▼");
    // A click on it: half a screen down, the session's line still at the
    // top and the tabs above counted under it.
    client.send(format!("\x1b[<0;6;{}M\x1b[<0;6;{}m", down + 1, down + 1).as_bytes());
    let screen = client.wait_for("tabs above counted", |s| {
        left(&s[1], 34).contains("▲") && left(&s[1], 34).contains("more")
    });
    assert!(left(&screen[0], 34).contains("main"), "{:?}", screen[0]);
    let list = wait_list(&server, "the sidebar focused, on a later tab", |l| {
        focused(l) && tabs(l)[0]["current"] == false
    });
    assert!(focused(&list));
    // A click on the ▲ row goes back up.
    client.send(b"\x1b[<0;6;2M\x1b[<0;6;2m");
    client.wait_for("back at the top", |s| !left(&s[1], 34).contains("▲"));
}

#[test]
fn d_and_l_in_the_sidebar_put_a_tool_in_the_chosen_tab_s_pane_s_place() {
    let server = TestServer::start();
    settings(&server, json!({}));
    // A stand-in lazygit, as tests/diff.rs has.
    stand_in::lazygit(&server.home.join("bin"));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    let first = tabs(&server.list())[0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("2:second");

    // L on the first tab: gone to, lazygit in its pane's place.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("1");
    client.type_text("L");
    client.wait_text("stand-in lazygit");
    let list = wait_list(&server, "lazygit in the first tab's pane's place", |l| {
        tabs(l)[0]["current"] == true && tabs(l)[0]["panes"][0]["options"]["@in_place"] == "lazygit"
    });
    assert_eq!(
        tabs(&list)[0]["panes"][0]["options"]["@parked_pane"],
        first.as_str()
    );
    assert!(!focused(&list), "the focus in the tool");
}

#[test]
fn a_tab_whose_claude_is_behind_a_tool_in_its_pane_s_place_keeps_its_stage() {
    let server = TestServer::start();
    settings(&server, json!({}));
    stand_in::lazygit(&server.home.join("bin"));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    // The first tab's shell stands in for a Claude, lazygit in its place.
    let (pane, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "s-parked"}));
    server.ok(&["press", "lazygit"]);
    client.wait_text("stand-in lazygit");
    wait_list(&server, "lazygit in the pane's place", |l| {
        tabs(l)[0]["panes"][0]["options"]["@in_place"] == "lazygit"
    });
    // At work behind it: under Working, not Other.
    hook(&server, &pane, "UserPromptSubmit", json!({}));
    let screen = client.wait_for("Working", |s| {
        s.iter().any(|l| left(l, 34).contains("WORKING 1"))
    });
    assert!(
        !screen.iter().any(|l| left(l, 34).contains("OTHER")),
        "{}",
        screen.join("\n")
    );
    // Finished, and seen there: under Idle.
    hook(&server, &pane, "Stop", json!({}));
    let screen = client.wait_for("Idle", |s| s.iter().any(|l| left(l, 34).contains("IDLE 1")));
    assert!(
        !screen.iter().any(|l| left(l, 34).contains("OTHER")),
        "{}",
        screen.join("\n")
    );

    // From another tab, Enter on it in the sidebar goes to it with the
    // focus in lazygit's pane, the Claude still parked behind it.
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("2:second");
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("1");
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    client.type_text("\r");
    let list = wait_list(&server, "the focus in the pane", |l| {
        !focused(l) && tabs(l)[0]["current"] == true
    });
    let shown = &tabs(&list)[0]["panes"];
    assert_eq!(shown.as_array().unwrap().len(), 1, "{shown}");
    assert_eq!(shown[0]["options"]["@in_place"], "lazygit");
    assert_eq!(shown[0]["options"]["@parked_pane"], pane.as_str());
    assert_eq!(shown[0]["active"], true, "the focus in lazygit: {shown}");
}

#[test]
fn a_claude_behind_a_tool_opened_over_another_tool_keeps_its_tab_in_its_stage() {
    let server = TestServer::start();
    settings(&server, json!({}));
    stand_in::lazygit(&server.home.join("bin"));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    // The diff opens only in a repository.
    let repo = server.home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init"]);
    git(&repo, &["commit", "--allow-empty", "-m", "first"]);
    cd(&server, &mut client, &repo);
    let (pane, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "s-stacked"}));
    // lazygit in the Claude's place, then the diff in lazygit's.
    server.ok(&["press", "lazygit"]);
    wait_list(&server, "lazygit in the pane's place", |l| {
        tabs(l)[0]["panes"][0]["options"]["@in_place"] == "lazygit"
    });
    server.ok(&["press", "diff-working-tree"]);
    let list = wait_list(&server, "the diff in lazygit's place", |l| {
        let shown = &tabs(l)[0]["panes"][0]["options"]["@in_place"];
        shown.is_string() && shown != "lazygit"
    });
    let diff = tabs(&list)[0]["panes"][0]["id"].clone();
    hook(&server, &pane, "UserPromptSubmit", json!({}));
    let screen = client.wait_for("Working", |s| {
        s.iter().any(|l| left(l, 34).contains("WORKING 1"))
    });
    assert!(
        !screen.iter().any(|l| left(l, 34).contains("OTHER")),
        "{}",
        screen.join("\n")
    );

    // Gone to from the sidebar, the focus is in the diff, the one drawn.
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("2:second");
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("1");
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    client.type_text("\r");
    let list = wait_list(&server, "the focus in the pane", |l| {
        !focused(l) && tabs(l)[0]["current"] == true
    });
    let shown = &tabs(&list)[0]["panes"];
    assert_eq!(shown.as_array().unwrap().len(), 1, "{shown}");
    assert_eq!(shown[0]["id"], diff);
    assert_eq!(shown[0]["active"], true, "the focus in the diff: {shown}");
}

#[test]
fn a_reply_sent_to_a_tab_whose_claude_is_behind_a_tool_brings_the_claude_beside_it() {
    let server = TestServer::start();
    settings(&server, json!({}));
    stand_in::lazygit(&server.home.join("bin"));
    let mut client = server.attach(140, 30);
    client.wait_text("$");
    // The first tab's Claude, lazygit in its place.
    let (dest, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "s-dest"}));
    server.ok(&["press", "lazygit"]);
    wait_list(&server, "lazygit in the pane's place", |l| {
        tabs(l)[0]["panes"][0]["options"]["@in_place"] == "lazygit"
    });
    // A second tab's Claude, with a reply in its transcript.
    server.ok(&["new-tab", "-n", "from"]);
    client.wait_text("2:from");
    let from = wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
    let from_pid = tabs(&from)[1]["panes"][0]["pid"].as_u64().unwrap();
    session_file(&server.home, from_pid, json!({"sessionId": "s-from"}));
    let project = server.home.join(".claude/projects/p");
    std::fs::create_dir_all(&project).unwrap();
    let at = jiff::Timestamp::now().to_string();
    let lines = [
        json!({"type": "user", "timestamp": at, "message": {"content": "go"}}),
        json!({"type": "assistant", "timestamp": at, "message": {"content": [
            {"type": "text", "text": "reply-to-pass-on"}
        ]}}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(project.join("s-from.jsonl"), text).unwrap();
    // Both tabs' Claudes read: neither is under Other.
    client.wait_for("both tabs under Idle", |s| {
        s.iter().any(|l| left(l, 34).contains("IDLE 2"))
    });

    server.ok(&["press", "send-reply"]);
    client.wait_text("send to>");
    client.type_text("\r");
    // The first tab in front, its Claude beside lazygit with the focus and
    // the reply at its prompt, unsent.
    let list = wait_list(&server, "the Claude beside lazygit, focused", |l| {
        let panes = tabs(l)[0]["panes"].as_array().cloned().unwrap_or_default();
        tabs(l)[0]["current"] == true
            && panes.len() == 2
            && panes
                .iter()
                .any(|p| p["id"] == dest.as_str() && p["active"] == true)
    });
    assert!(
        tabs(&list)[0]["panes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["options"]["@in_place"] == "lazygit"),
        "lazygit still there: {list}"
    );
    client.wait_text("reply-to-pass-on");
}

/// The Notifications window's tab, by its place in the list, if made.
fn notices_tab(list: &Value) -> Option<usize> {
    tabs(list)
        .iter()
        .position(|t| t["options"]["@notices_window"] == "1")
}

#[test]
fn the_cursor_onto_a_notification_brings_the_notifications_window_to_the_front() {
    // A note written for you is a notification, as the meeting notes give one.
    let notes = common::TempDir::new("sbnw");
    std::fs::write(notes.path().join("Plan.md"), "# Plan\n").unwrap();
    let server = TestServer::start_with_env(&[("NOTES_DIR", notes.path().to_str().unwrap())]);
    settings(&server, json!({}));
    std::fs::create_dir_all(&server.state).unwrap();
    std::fs::write(
        server.state.join("new-notes.json"),
        json!([{ "path": "Plan.md", "title": "Plan", "source": "Fathom", "at": 1 }]).to_string(),
    )
    .unwrap();
    let mut client = server.attach(140, 36);
    client.wait_text("● New note: Plan");

    // M-s, then n: the cursor onto it, and the window in front, the focus
    // still in the sidebar, the notification's whole message in it.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("n");
    let list = wait_list(&server, "the Notifications window in front", |l| {
        notices_tab(l).is_some_and(|i| tabs(l)[i]["current"] == true) && focused(l)
    });
    assert_eq!(notices_tab(&list), Some(1), "last, after the one tab");
    let screen = client.wait_text("NOTIFICATIONS 1");
    let has = |text: &str| screen.iter().any(|l| l.contains(text));
    assert!(has("‹ tabs"), "{}", screen.join("\n"));
    assert!(has("▌ Plan"));
    assert!(has("Written from Fathom into Plan.md"), "whole");
    let bar = screen.last().unwrap();
    assert!(
        bar.contains("Notifications"),
        "in the tab bar while in front: {bar}"
    );
    // No tab in the sidebar or mission control: one tab under the heading.
    assert!(
        screen.iter().any(|l| l.starts_with(" OTHER 1")),
        "{}",
        screen.join("\n")
    );

    // k: back up to the tab, which comes to the front, the window out of
    // the tab bar.
    client.type_text("k");
    wait_list(&server, "the tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    let screen = client.wait_for("the window out of the bar", |s| {
        !s.last().unwrap().contains("Notifications")
    });
    assert!(screen.iter().any(|l| l.contains("● New note: Plan")));

    // Enter on the notification: into the window's pane; Enter there opens
    // its menu over it, Esc closes that, and Esc goes back to the tab.
    client.type_text("j");
    wait_list(&server, "the window again", |l| {
        notices_tab(l).is_some_and(|i| tabs(l)[i]["current"] == true)
    });
    client.type_text("\r");
    wait_list(&server, "the focus in the window", |l| {
        !focused(l) && notices_tab(l).is_some_and(|i| tabs(l)[i]["current"] == true)
    });
    client.type_text("\r");
    client.wait_text("Open it in Notes");
    client.type_text("\x1b");
    client.wait_for("the menu closed", |s| {
        !s.iter().any(|l| l.contains("Open it in Notes"))
    });
    client.type_text("\x1b");
    wait_list(&server, "back on the tab it came from", |l| {
        tabs(l)[0]["current"] == true && !focused(l)
    });
    // A new tab goes before the window, and `switchboard tabs` leaves it out.
    server.ok(&["new-tab", "-n", "second"]);
    let list = wait_list(&server, "the second tab", |l| tabs(l).len() == 3);
    assert_eq!(notices_tab(&list), Some(2), "kept last");
    let (code, out, _) = server.run(&["tabs"]);
    assert_eq!(code, 0);
    assert!(!out.contains("Notifications"), "no tab for `tabs`: {out}");
}

#[test]
fn with_ux2_k_from_the_first_tab_goes_up_onto_the_notifications() {
    let notes = common::TempDir::new("sbnu");
    std::fs::write(notes.path().join("Plan.md"), "# Plan\n").unwrap();
    // UX 2.0 from the start: the notification drawn before the server read
    // a later setting would be the classic sidebar's, below the tab.
    let server = TestServer::start_with_env_and_settings(
        &[("NOTES_DIR", notes.path().to_str().unwrap())],
        r#"{"notifications": false, "tips": false, "ux2": true}"#,
    );
    std::fs::create_dir_all(&server.state).unwrap();
    std::fs::write(
        server.state.join("new-notes.json"),
        json!([{ "path": "Plan.md", "title": "Plan", "source": "Fathom", "at": 1 }]).to_string(),
    )
    .unwrap();
    let mut client = server.attach(140, 36);
    client.wait_text("New note: Plan");

    // j from the only tab stays on it: the notifications are above.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("j");
    std::thread::sleep(Duration::from_millis(300));
    let list = wait_list(&server, "the tab still in front", focused);
    assert_eq!(notices_tab(&list), None, "no window opened");

    // k: up onto the notification, its window in front.
    client.type_text("k");
    wait_list(&server, "the Notifications window in front", |l| {
        notices_tab(l).is_some_and(|i| tabs(l)[i]["current"] == true) && focused(l)
    });

    // k again stays; j goes back down to the tab.
    client.type_text("k");
    client.type_text("j");
    wait_list(&server, "the tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
}

/// Runs git in `dir` with an identity of its own and no config of yours.
fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// The first tab's pane in `dir`, once its shell has gone there.
fn cd(server: &TestServer, client: &mut TestClient, dir: &Path) {
    client.type_text(&format!("cd '{}'\r", dir.display()));
    let want = dir.canonicalize().unwrap();
    wait_list(server, "the shell to cd", |l| {
        tabs(l)[0]["panes"][0]["cwd"]
            .as_str()
            .and_then(|d| Path::new(d).canonicalize().ok())
            .is_some_and(|d| d == want)
    });
}

#[test]
fn a_tab_whose_work_landed_straight_on_a_branch_is_under_merged_while_in_its_main_clone() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let clone = server.dir.path().join("clone");
    std::fs::create_dir_all(&clone).unwrap();
    git(&clone, &["init", "-q"]);
    git(&clone, &["commit", "-q", "--allow-empty", "-m", "start"]);
    let worktree = server.dir.path().join("next");
    git(
        &clone,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "next",
            &worktree.to_string_lossy(),
        ],
    );
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    let (pane, _) = first_pane(&server);
    let tab = tabs(&server.list())[0]["id"].as_str().unwrap().to_string();
    let land_lock = |dir: &Path, args: &[&str]| {
        Command::new(common::BIN)
            .arg("land-lock")
            .args(args)
            .current_dir(dir)
            .env("HOME", &server.home)
            .env("SWITCHBOARD_STATE_DIR", &server.state)
            .env("SWITCHBOARD", server.socket())
            .env("SWITCHBOARD_PANE", &pane)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    assert_eq!(
        land_lock(&worktree, &["landed", "abc1234"]).status.code(),
        Some(2)
    );
    // Landed from the worktree, as queue-and-clean pushes from it.
    let said = land_lock(&worktree, &["landed", "abc1234", "ux-2-0"]);
    assert!(
        said.status.success(),
        "{}",
        String::from_utf8_lossy(&said.stderr)
    );
    let repo = clone.join(".git").canonicalize().unwrap();
    assert_eq!(
        server.ok(&["show", "-t", &tab, "@tab_landed"]).trim(),
        format!("abc1234 ux-2-0 main {}", repo.display())
    );
    // Back in the main clone, as queue-and-clean leaves the tab.
    cd(&server, &mut client, &clone);
    let at = row_of(&mut client, "MERGED 1");
    assert!(row_of(&mut client, "landed on ux-2-0") > at);
    // Gone on to a worktree, however Claude starts there.
    cd(&server, &mut client, &worktree);
    client.wait_for("Merged gone", |s| {
        !s.iter().any(|l| left(l, 34).contains("MERGED"))
    });
    // Back, it is Merged again; a new branch in the main clone is new work.
    cd(&server, &mut client, &clone);
    row_of(&mut client, "MERGED 1");
    git(&clone, &["checkout", "-q", "-b", "new-work"]);
    client.wait_for("Merged gone on a new branch", |s| {
        !s.iter().any(|l| left(l, 34).contains("MERGED"))
    });
    // Back on its branch, it is Merged until a prompt starts new work there.
    git(&clone, &["checkout", "-q", "main"]);
    row_of(&mut client, "MERGED 1");
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        json!({"prompt": "next"}),
    );
    client.wait_for("Merged gone on a prompt", |s| {
        !s.iter().any(|l| left(l, 34).contains("MERGED"))
    });
    assert_eq!(server.ok(&["show", "-t", &tab, "@tab_landed"]).trim(), "");
}

// The window's menu on a Slack message is the Slack view's: the message
// above the choices, its dismissing worded as there.
#[test]
fn a_slack_message_s_menu_in_the_notifications_window_is_the_slack_view_s() {
    let server = TestServer::start();
    settings(&server, json!({}));
    std::fs::create_dir_all(&server.state).unwrap();
    // Sent now: a dismissal is kept two days past its message.
    let ts = format!("{}.000100", switchboard::util::now_ms() / 1000);
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": 1, "items": [{
            "kind": "dm", "channel": "D1", "from": "Ada", "where": "Ada",
            "text": "its first line\nits second line\nits third line",
            "ts": ts, "link": "https://slack.example/m",
        }] })
        .to_string(),
    )
    .unwrap();
    let mut client = server.attach(140, 36);
    client.wait_text("1 new on Slack");
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("n");
    wait_list(&server, "the Notifications window in front", |l| {
        notices_tab(l).is_some_and(|i| tabs(l)[i]["current"] == true) && focused(l)
    });
    client.type_text("\r");
    wait_list(&server, "the focus in the window", |l| !focused(l));
    client.type_text("\r");
    let screen = client.wait_text("Dismiss it, until its thread gets a reply");
    let all = screen.join("\n");
    let choices = screen
        .iter()
        .position(|l| l.contains("Open it in Slack"))
        .unwrap_or_else(|| panic!("{all}"));
    assert!(
        screen[..choices]
            .iter()
            .any(|l| l.contains("its third line")),
        "the message above the choices: {all}"
    );
    assert!(all.contains("Open a tab for this thread with Claude"));
    assert!(!all.contains("Dismiss every message shown"), "{all}");
    assert!(
        !all.replace("Dismiss it, until", "").contains("Dismiss it"),
        "one way to dismiss it: {all}"
    );
    // Taken, it dismisses the message, and the notification with it.
    client.type_text("jj\r");
    client.wait_for("the notification gone", |s| {
        !s.iter().any(|l| l.contains("1 new on Slack"))
    });
    let gone = std::fs::read_to_string(server.state.join("slack-dismissed.json")).unwrap();
    assert!(gone.contains(&format!("D1:{ts}")), "{gone}");
}
