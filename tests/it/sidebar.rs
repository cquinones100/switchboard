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

/// The index of the tab in front.
fn in_front(list: &Value) -> usize {
    tabs(list)
        .iter()
        .position(|t| t["current"] == true)
        .unwrap()
}

/// Eight tabs on a short screen, the first in front.
fn short_screen_of_tabs(server: &TestServer) -> TestClient {
    let mut client = server.attach(120, 14);
    client.wait_text("$");
    for n in 2..=8 {
        server.ok(&["new-tab", "-d", "-n", &format!("t{n}")]);
    }
    server.ok(&[
        "select-tab",
        "-t",
        tabs(&server.list())[0]["id"].as_str().unwrap(),
    ]);
    client.wait_for("tabs cut below", |s| {
        s.iter().any(|l| left(l, 34).contains("▼"))
    });
    client
}

#[test]
fn the_wheel_over_the_sidebar_moves_half_a_screen_once_a_flick() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = short_screen_of_tabs(&server);
    // One wheel-down: half a screen on, as C-d, the focus in the sidebar.
    client.send(b"\x1b[<65;6;5M");
    let list = wait_list(&server, "half a screen down", |l| {
        focused(l) && in_front(l) > 1
    });
    let half = in_front(&list);
    client.wait_for("tabs above counted", |s| left(&s[1], 34).contains("▲"));
    // One wheel-up, the other way: back to the first, as C-u.
    client.send(b"\x1b[<64;6;5M");
    wait_list(&server, "back at the first", |l| in_front(l) == 0);
    client.wait_for("back at the top", |s| !left(&s[1], 34).contains("▲"));
    // A flick of five: half a screen once.
    client.send("\x1b[<65;6;5M".repeat(5).as_bytes());
    wait_list(&server, "half a screen down again", |l| in_front(l) > 0);
    std::thread::sleep(Duration::from_millis(100) * common::patience());
    assert_eq!(in_front(&server.list()), half);
}

#[test]
fn with_half_page_scrolling_off_the_wheel_moves_a_tab_at_a_time() {
    // Off from the start, so the wheel never meets it still on.
    let server = TestServer::start_with_settings(
        r#"{"notifications": false, "tips": false, "sidebar_half_page": false}"#,
    );
    let mut client = short_screen_of_tabs(&server);
    // One wheel-down moves the cursor one tab, as j does, the list not
    // scrolled: to t2, the next by number, though t8 opened beside the first.
    client.send(b"\x1b[<65;6;5M");
    wait_list(&server, "the next tab", |l| focused(l) && in_front(l) > 0);
    std::thread::sleep(Duration::from_millis(100) * common::patience());
    let list = server.list();
    assert_eq!(tabs(&list)[in_front(&list)]["name"], "t2");
    let screen = client.wait_for("the cursor on t2", |s| {
        s.iter().any(|l| left(l, 34).contains("▌ ○  2 t2"))
    });
    assert!(!left(&screen[1], 34).contains("▲"), "{}", screen.join("\n"));
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
    // m opens the view's own menu, as in mission control's view.
    client.type_text("m");
    client.wait_text("Dismiss every notification shown");
    client.type_text("\x1b");
    client.wait_for("the view's menu closed", |s| {
        !s.iter()
            .any(|l| l.contains("Dismiss every notification shown"))
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

/// Two Slack messages and a note written for you, in the state and notes
/// directories of a server started with `settings`.
fn two_kinds_of_notification(settings: &str) -> (TestServer, common::TempDir) {
    let notes = common::TempDir::new("sbnk");
    std::fs::write(notes.path().join("Plan.md"), "# Plan\n").unwrap();
    let server = TestServer::start_with_env_and_settings(
        &[("NOTES_DIR", notes.path().to_str().unwrap())],
        settings,
    );
    std::fs::create_dir_all(&server.state).unwrap();
    std::fs::write(
        server.state.join("new-notes.json"),
        json!([{ "path": "Plan.md", "title": "Plan", "source": "Fathom", "at": 1 }]).to_string(),
    )
    .unwrap();
    let now = switchboard::util::now_ms() / 1000;
    let message = |from: &str, text: &str, ts: String| {
        json!({
            "kind": "dm", "channel": "D1", "from": from, "where": from,
            "text": text, "ts": ts, "link": "https://slack.example/m",
        })
    };
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": switchboard::util::now_ms(), "items": [
            message("Ada", "first-message", format!("{now}.000100")),
            message("Grace", "second-message", format!("{now}.000200")),
        ] })
        .to_string(),
    )
    .unwrap();
    (server, notes)
}

/// The Notifications window in front, its chosen notification's line
/// holding `chosen`.
fn wait_chosen(client: &mut TestClient, chosen: &str) {
    client.wait_for(&format!("{chosen} chosen in the window"), |s| {
        s.iter().any(|l| l.contains("│▌") && l.contains(chosen))
    });
}

#[test]
fn j_and_k_in_the_sidebar_step_over_the_notifications_a_kind_at_a_time() {
    let (server, _notes) = two_kinds_of_notification(r#"{"notifications": false, "tips": false}"#);
    let mut client = server.attach(140, 36);
    client.wait_text("2 new on Slack");
    client.wait_text("New note: Plan");

    // j from the tab: onto the Slack line, the window on its first message.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("j");
    wait_list(&server, "the Notifications window in front", |l| {
        notices_tab(l).is_some_and(|i| tabs(l)[i]["current"] == true) && focused(l)
    });
    wait_chosen(&mut client, "DM Ada");
    // One more j: past the second message, onto the note's line.
    client.type_text("j");
    wait_chosen(&mut client, "Plan");
    // k: back onto the Slack line, on its first message again.
    client.type_text("k");
    wait_chosen(&mut client, "DM Ada");
    // k again: back up to the tab.
    client.type_text("k");
    wait_list(&server, "the tab in front", |l| {
        tabs(l)[0]["current"] == true && focused(l)
    });

    // Inside the window, j still chooses one by one.
    client.type_text("j");
    wait_chosen(&mut client, "DM Ada");
    client.type_text("\r");
    wait_list(&server, "the focus in the window", |l| !focused(l));
    client.type_text("j");
    wait_chosen(&mut client, "DM Grace");
}

#[test]
fn j_and_k_in_the_notifications_window_wrap_from_its_last_to_its_first_and_back() {
    let (server, _notes) = two_kinds_of_notification(r#"{"notifications": false, "tips": false}"#);
    let mut client = server.attach(140, 36);
    client.wait_text("2 new on Slack");
    client.wait_text("New note: Plan");
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("j");
    wait_chosen(&mut client, "DM Ada");
    client.type_text("\r");
    wait_list(&server, "the focus in the window", |l| !focused(l));
    client.type_text("G");
    wait_chosen(&mut client, "Plan");
    client.type_text("j");
    wait_chosen(&mut client, "DM Ada");
    client.type_text("k");
    wait_chosen(&mut client, "Plan");
}

#[test]
fn the_wheel_past_the_last_tab_steps_over_the_notifications_as_j_does() {
    let (server, _notes) = two_kinds_of_notification(r#"{"notifications": false, "tips": false}"#);
    let mut client = server.attach(140, 36);
    client.wait_text("2 new on Slack");
    client.wait_text("New note: Plan");

    // Down from the only tab: onto the Slack line, as j.
    client.send(b"\x1b[<65;6;5M");
    wait_list(&server, "the Notifications window in front", |l| {
        notices_tab(l).is_some_and(|i| tabs(l)[i]["current"] == true) && focused(l)
    });
    wait_chosen(&mut client, "DM Ada");
    // After the flick's pause, down again: the note's line.
    std::thread::sleep(Duration::from_millis(300));
    client.send(b"\x1b[<65;6;5M");
    wait_chosen(&mut client, "Plan");
    // Up: back onto the Slack line.
    client.send(b"\x1b[<64;6;5M");
    wait_chosen(&mut client, "DM Ada");
}

#[test]
fn with_ux2_j_from_the_last_kind_of_notification_goes_onto_the_first_tab() {
    let (server, _notes) =
        two_kinds_of_notification(r#"{"notifications": false, "tips": false, "ux2": true}"#);
    let mut client = server.attach(140, 36);
    client.wait_text("2 new on Slack");
    client.wait_text("New note: Plan");

    // k from the tab, past its group's heading: onto the last kind, the
    // note's.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("kk");
    wait_list(&server, "the Notifications window in front", |l| {
        notices_tab(l).is_some_and(|i| tabs(l)[i]["current"] == true) && focused(l)
    });
    wait_chosen(&mut client, "Plan");
    // k: onto the Slack line's first message, past the second.
    client.type_text("k");
    wait_chosen(&mut client, "DM Ada");
    // j, j, j: the note, the first group's heading, then the first tab.
    client.type_text("j");
    wait_chosen(&mut client, "Plan");
    client.type_text("jj");
    wait_list(&server, "the tab in front", |l| {
        tabs(l)[0]["current"] == true && focused(l)
    });
}

#[test]
fn with_ux2_k_from_the_first_tab_goes_up_onto_the_notifications() {
    let notes = common::TempDir::new("sbnu");
    std::fs::write(notes.path().join("Plan.md"), "# Plan\n").unwrap();
    // UX 2.0 from the start: the notification drawn before the server read
    // a later setting would be the classic sidebar's, below the tab.
    let server = TestServer::start_with_env_and_settings(
        &[("NOTES_DIR", notes.path().to_str().unwrap())],
        r#"{"notifications": false, "tips": false, "ux2": true, "sidebar_wrap": false}"#,
    );
    std::fs::create_dir_all(&server.state).unwrap();
    std::fs::write(
        server.state.join("new-notes.json"),
        json!([{ "path": "Plan.md", "title": "Plan", "source": "Fathom", "at": 1 }]).to_string(),
    )
    .unwrap();
    let mut client = server.attach(140, 36);
    client.wait_text("New note: Plan");

    // With wrapping off, j from the only tab stays on it: the
    // notifications are above.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("j");
    std::thread::sleep(Duration::from_millis(300));
    let list = wait_list(&server, "the tab still in front", focused);
    assert_eq!(notices_tab(&list), None, "no window opened");

    // k, k: up past its group's heading onto the notification, its window
    // in front.
    client.type_text("kk");
    wait_list(&server, "the Notifications window in front", |l| {
        notices_tab(l).is_some_and(|i| tabs(l)[i]["current"] == true) && focused(l)
    });

    // k again stays; j, j go back down past the heading to the tab.
    client.type_text("k");
    client.type_text("jj");
    wait_list(&server, "the tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
}

#[test]
fn up_and_down_wrap_around_the_tabs() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    server.ok(&["new-tab", "-n", "third"]);
    client.wait_text("3:third");
    wait_list(&server, "the third tab in front", |l| {
        tabs(l)[2]["current"] == true
    });

    // j on the last tab: round to the group's heading, then the first tab;
    // k on the first: past the heading round to the last.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("jj");
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true && focused(l)
    });
    client.type_text("kk");
    wait_list(&server, "the last tab in front", |l| {
        tabs(l)[2]["current"] == true && focused(l)
    });
}

#[test]
fn with_wrapping_off_up_and_down_stop_at_the_ends() {
    let server = TestServer::start_with_settings(
        r#"{"notifications": false, "tips": false, "sidebar_wrap": false}"#,
    );
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    server.ok(&["new-tab", "-n", "third"]);
    client.wait_text("3:third");
    wait_list(&server, "the third tab in front", |l| {
        tabs(l)[2]["current"] == true
    });

    // j on the last stays, so k goes up to the second rather than staying
    // on the first.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("j");
    client.type_text("k");
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true && focused(l)
    });
}

#[test]
fn the_wheel_stops_at_the_last_tab_while_the_keys_wrap() {
    let server = TestServer::start_with_settings(
        r#"{"notifications": false, "tips": false, "sidebar_half_page": false}"#,
    );
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    server.ok(&["new-tab", "-n", "third"]);
    client.wait_text("3:third");
    wait_list(&server, "the third tab in front", |l| {
        tabs(l)[2]["current"] == true
    });

    // Down on the last tab stays, so up goes to the second rather than
    // staying on the first.
    client.send(b"\x1b[<65;6;5M");
    wait_list(&server, "the sidebar focused", focused);
    client.send(b"\x1b[<64;6;5M");
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true && focused(l)
    });
}

#[test]
fn j_from_the_last_kind_of_notification_goes_round_to_the_first_tab() {
    let (server, _notes) = two_kinds_of_notification(r#"{"notifications": false, "tips": false}"#);
    let mut client = server.attach(140, 36);
    client.wait_text("2 new on Slack");
    client.wait_text("New note: Plan");

    // k from the only tab, past its group's heading: round to the last
    // kind, the note's.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("kk");
    wait_list(&server, "the Notifications window in front", |l| {
        notices_tab(l).is_some_and(|i| tabs(l)[i]["current"] == true) && focused(l)
    });
    wait_chosen(&mut client, "Plan");
    // j, j: round to the heading, then the tab.
    client.type_text("jj");
    wait_list(&server, "the tab in front", |l| {
        tabs(l)[0]["current"] == true && focused(l)
    });
    // j, j, j, j: Slack, the note, and round past the heading to the tab
    // again.
    client.type_text("j");
    wait_chosen(&mut client, "DM Ada");
    client.type_text("j");
    wait_chosen(&mut client, "Plan");
    client.type_text("jj");
    wait_list(&server, "the tab in front again", |l| {
        tabs(l)[0]["current"] == true && focused(l)
    });
}

// With UX 2.0 the wheel goes up from the first tab past its group's open
// heading onto the notifications above it, a flick at a time.
#[test]
fn with_ux2_the_wheel_goes_up_past_the_first_heading_onto_the_notifications() {
    let (server, _notes) =
        two_kinds_of_notification(r#"{"notifications": false, "tips": false, "ux2": true}"#);
    let mut client = server.attach(140, 36);
    client.wait_text("2 new on Slack");
    client.wait_text("New note: Plan");
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    let deadline = common::deadline(Duration::from_secs(10));
    loop {
        client.send(b"\x1b[<64;6;20M");
        std::thread::sleep(Duration::from_millis(300) * common::patience());
        let list = server.list();
        if notices_tab(&list).is_some_and(|i| tabs(&list)[i]["current"] == true) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the wheel never reached the notifications: {list}"
        );
    }
}

#[test]
fn with_ux2_j_from_the_last_tab_goes_round_to_the_first_kind_of_notification() {
    let (server, _notes) =
        two_kinds_of_notification(r#"{"notifications": false, "tips": false, "ux2": true}"#);
    let mut client = server.attach(140, 36);
    client.wait_text("2 new on Slack");
    client.wait_text("New note: Plan");

    // j from the only tab: round to the top, the Slack line.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("j");
    wait_list(&server, "the Notifications window in front", |l| {
        notices_tab(l).is_some_and(|i| tabs(l)[i]["current"] == true) && focused(l)
    });
    wait_chosen(&mut client, "DM Ada");
    // k from the first kind: round to the bottom, the tab.
    client.type_text("k");
    wait_list(&server, "the tab in front", |l| {
        tabs(l)[0]["current"] == true && focused(l)
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
    for choice in ["Open a tab for it", "Send it to a tab", "Copy its link"] {
        assert!(all.contains(choice), "{choice}: {all}");
    }
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

#[test]
fn list_gives_each_tab_its_stage_as_the_sidebar_groups_it() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "s-1"}));
    server.ok(&["new-tab", "-n", "later"]);
    client.wait_text("2:later");
    let second = tabs(&server.list())[1]["id"].as_str().unwrap().to_string();
    let at = jiff::Timestamp::now().as_second() + 3600;
    server.ok(&["set", "-t", &second, "@remind_at", &at.to_string()]);
    let ask = json!({"tool_name": "AskUserQuestion", "tool_use_id": "q1", "tool_input": {"questions": [{"question": "Push it?"}]}});
    hook(&server, &pane, "PreToolUse", ask);
    let list = wait_list(&server, "both stages", |l| {
        tabs(l)[0]["stage"]["key"] == "needs" && tabs(l)[1]["stage"]["key"] == "later"
    });
    let (first, later) = (&tabs(&list)[0]["stage"], &tabs(&list)[1]["stage"]);
    assert_eq!(first["title"], "Needs you");
    assert_eq!(later["title"], "Reminders");
    assert_eq!(
        (first["rank"].as_u64(), first["place"].as_u64()),
        (Some(0), Some(0))
    );
    assert!(later["rank"].as_u64() > first["rank"].as_u64(), "{list}");
}

// `sidebar-repo` filters the sidebar to one repository: the other's tab
// folded into one Other repos line with its mark, count and repository,
// unfolded with Enter there; the top line's chip names the clone and a click
// on it clears the filter, as M-p's "Show every repository's tabs" does; the
// filter outlives an upgrade; and with the setting off there is none.
#[test]
fn the_sidebar_filtered_to_one_repository_folds_the_others_under_other_repos() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let alpha = server.dir.path().join("alpha");
    let beta = server.dir.path().join("beta");
    for dir in [&alpha, &beta] {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q"]);
        git(dir, &["commit", "-q", "--allow-empty", "-m", "start"]);
    }
    let alpha = alpha.canonicalize().unwrap();
    let beta = beta.canonicalize().unwrap();
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    cd(&server, &mut client, &alpha);
    server.ok(&["new-tab", "-d", "-c", &beta.to_string_lossy(), "-n", "bee"]);
    row_of(&mut client, "bee");

    server.ok(&["sidebar-repo", &alpha.to_string_lossy()]);
    let repo = |l: &Value| l["sessions"][0]["sidebar"]["repo"].clone();
    wait_list(&server, "the filter set", |l| {
        repo(l) == json!(alpha.to_string_lossy())
    });
    let fold = row_of(&mut client, "1 tab · beta");
    assert!(row_of(&mut client, "OTHER REPOS 1") < fold);
    let screen = client.screen();
    assert!(left(&screen[0], 34).contains("alpha ×"), "{:?}", screen[0]);
    assert!(left(&screen[0], 34).contains("1 of 2"), "{:?}", screen[0]);
    assert!(
        !screen.iter().any(|l| left(l, 34).contains("bee")),
        "{screen:?}"
    );

    // Into the sidebar, down onto the folded line, and Enter unfolds it.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("j");
    client.type_text("\r");
    wait_list(&server, "Other repos unfolded", |l| {
        l["sessions"][0]["sidebar"]["repo_open"] == true
    });
    // Its tab is under the cursor but stays behind the one in front, until
    // Enter goes into it.
    row_of(&mut client, "bee");
    let list = server.list();
    assert!(
        tabs(&list)[0]["current"] == true && focused(&list),
        "{list}"
    );
    client.type_text("\r");
    wait_list(&server, "into the unfolded tab", |l| {
        tabs(l)[1]["current"] == true && !focused(l)
    });

    // A click on the chip clears the filter.
    client.send(b"\x1b[<0;4;1M\x1b[<0;4;1m");
    wait_list(&server, "the filter cleared", |l| repo(l).is_null());
    client.wait_for("Other repos gone", |s| {
        !s.iter().any(|l| left(l, 34).contains("OTHER REPOS"))
    });

    // M-p's "Show every repository's tabs" clears it too.
    server.ok(&["sidebar-repo", &beta.to_string_lossy()]);
    row_of(&mut client, "beta ×");
    server.ok(&["press", "sidebar-all"]);
    wait_list(&server, "cleared again", |l| repo(l).is_null());

    // Kept through an upgrade.
    server.ok(&["sidebar-repo", &alpha.to_string_lossy()]);
    let (code, out, err) = server.run(&["upgrade"]);
    assert_eq!(code, 0, "{out}{err}\nserver log: {}", server.log());
    wait_list(&server, "the filter kept", |l| {
        repo(l) == json!(alpha.to_string_lossy())
    });

    // A / search reaches a tab folded under Other repos.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("/bee");
    row_of(&mut client, "bee");
    client.type_text("\r");
    wait_list(&server, "the search's tab in front", |l| {
        tabs(l)[1]["current"] == true
            && !focused(l)
            && l["sessions"][0]["sidebar"]["repo_open"] == true
    });

    // A click on the folded line unfolds it, the tab in front kept.
    server.ok(&[
        "select-tab",
        "-t",
        tabs(&server.list())[0]["id"].as_str().unwrap(),
    ]);
    server.ok(&["sidebar-repo", &alpha.to_string_lossy()]);
    let fold = row_of(&mut client, "1 tab · beta");
    client.send(format!("\x1b[<0;6;{}M\x1b[<0;6;{}m", fold + 1, fold + 1).as_bytes());
    wait_list(&server, "unfolded by a click", |l| {
        l["sessions"][0]["sidebar"]["repo_open"] == true && focused(l)
    });
    let list = server.list();
    assert!(tabs(&list)[0]["current"] == true, "{list}");

    // A digit for a folded tab unfolds it, the cursor there in the sidebar.
    server.ok(&["sidebar-repo", &alpha.to_string_lossy()]);
    server.ok(&[
        "select-tab",
        "-t",
        tabs(&server.list())[0]["id"].as_str().unwrap(),
    ]);
    wait_list(&server, "folded again, the sidebar focused", |l| {
        l["sessions"][0]["sidebar"]["repo_open"] == false && focused(l)
    });
    client.type_text("2");
    wait_list(
        &server,
        "the digit's tab in front, still in the sidebar",
        |l| {
            tabs(l)[1]["current"] == true
                && focused(l)
                && l["sessions"][0]["sidebar"]["repo_open"] == true
        },
    );

    client.type_text("q");
    wait_list(&server, "out of the sidebar", |l| !focused(l));

    // With the setting off, the sidebar is as before and the command fails.
    settings(&server, json!({"sidebar_repo_filter": false}));
    client.wait_for("Other repos gone with the setting off", |s| {
        !s.iter().any(|l| left(l, 34).contains("OTHER REPOS"))
    });
    let (code, _, err) = server.run(&["sidebar-repo", "--all"]);
    assert_ne!(code, 0);
    assert!(err.contains("sidebar_repo_filter"), "{err}");
}

// Filtered to one repository, another's tab with a reminder come due is
// under Reminders elsewhere with its repository named, after the filtered
// repository's own under Needs you, which M-a goes to first; one without
// stays in Other repos, and once its reminder is done with it goes there
// too. With no filter there is no such group.
#[test]
fn a_reminder_come_due_shows_through_the_repository_filter() {
    let server = TestServer::start();
    // A reminder's box would take the M-a pressed under it.
    settings(&server, json!({"message_boxes": false}));
    let alpha = server.dir.path().join("alpha");
    let beta = server.dir.path().join("beta");
    for dir in [&alpha, &beta] {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q"]);
        git(dir, &["commit", "-q", "--allow-empty", "-m", "start"]);
    }
    let alpha = alpha.canonicalize().unwrap();
    let beta = beta.canonicalize().unwrap();
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    cd(&server, &mut client, &alpha);
    for name in ["bee", "wasp"] {
        server.ok(&["new-tab", "-d", "-c", &beta.to_string_lossy(), "-n", name]);
        row_of(&mut client, name);
    }
    let list = server.list();
    let bee = tabs(&list).iter().find(|t| t["name"] == "bee").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["set", "-t", &bee, "@remind_at", "1"]);
    server.ok(&["new-tab", "-d", "-c", &alpha.to_string_lossy(), "-n", "ant"]);
    row_of(&mut client, "ant");
    let list = server.list();
    let ant = tabs(&list).iter().find(|t| t["name"] == "ant").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["set", "-t", &ant, "@remind_at", "1"]);
    row_of(&mut client, "NEEDS YOU");
    assert!(
        !client
            .screen()
            .iter()
            .any(|l| left(l, 34).contains("REMINDERS ELSEWHERE")),
        "no such group with no filter"
    );

    server.ok(&["sidebar-repo", &alpha.to_string_lossy()]);
    let fold = row_of(&mut client, "1 tab · beta");
    let needs = row_of(&mut client, "NEEDS YOU");
    let mine = row_of(&mut client, "ant");
    let group = row_of(&mut client, "REMINDERS ELSEWHERE 1");
    let row = row_of(&mut client, "bee");
    assert!(needs < mine && mine < group && group < row && row < fold);
    let screen = client.screen();
    assert!(
        screen[row + 1..fold - 1]
            .iter()
            .any(|l| left(l, 34).contains("beta")),
        "{screen:?}"
    );
    assert!(left(&screen[0], 34).contains("2 of 4"), "{:?}", screen[0]);
    assert!(
        !screen.iter().any(|l| left(l, 34).contains("wasp")),
        "{screen:?}"
    );

    server.ok(&["seen", "-t", &bee]);
    row_of(&mut client, "2 tabs · beta");
    client.wait_for("bee folded away", |s| {
        !s.iter().any(|l| left(l, 34).contains("bee"))
    });

    server.ok(&["set", "-t", &bee, "@remind_at", "1"]);
    row_of(&mut client, "REMINDERS ELSEWHERE 1");
    // M-a: the filtered repository's reminder first, then the other's.
    let current = |server: &TestServer| {
        let list = server.list();
        tabs(&list)
            .iter()
            .find(|t| t["current"] == true)
            .map(|t| t["name"].as_str().unwrap_or_default().to_string())
    };
    client.type_text(&format!("{ALT}a"));
    wait_list(&server, "M-a at the filtered repository's tab", |_| {
        current(&server).as_deref() == Some("ant")
    });
    client.type_text(&format!("{ALT}a"));
    wait_list(&server, "M-a at the other repository's tab", |_| {
        current(&server).as_deref() == Some("bee")
    });
}

// M-R opens "Filter the sidebar to a repository": choosing a repository
// filters the sidebar to it, and M-R again offers "Every repository" first,
// above the repository it opens on, which typing its name and Enter takes,
// clearing the filter; M-p lists the command with its key.
#[test]
fn m_shift_r_filters_the_sidebar_to_a_repository() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let beta = server.dir.path().join("beta");
    {
        let dir = &beta;
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q"]);
        git(dir, &["commit", "-q", "--allow-empty", "-m", "start"]);
    }
    let beta = beta.canonicalize().unwrap();
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-d", "-c", &beta.to_string_lossy(), "-n", "bee"]);
    row_of(&mut client, "bee");
    let repo = |l: &Value| l["sessions"][0]["sidebar"]["repo"].clone();

    client.type_text(&format!("{ALT}R"));
    client.wait_text("Filter the sidebar to a repository");
    client.type_text("beta");
    client.wait_text("beta  1 tab");
    client.type_text("\r");
    wait_list(&server, "filtered to beta", |l| {
        repo(l) == json!(beta.to_string_lossy())
    });
    // Its box closes on any key, which goes no further.
    client.wait_text("Sidebar: beta only");
    client.type_text("x");
    client.wait_for("the box closed", |s| {
        !s.iter().any(|l| l.contains("Sidebar: beta only"))
    });

    client.type_text(&format!("{ALT}R"));
    let screen = client.wait_text("Filter the sidebar to a repository");
    let every = screen.iter().position(|l| l.contains("Every repository"));
    let first_repo = screen.iter().position(|l| l.contains("beta  1 tab"));
    assert!(every.is_some() && every < first_repo, "{screen:?}");
    client.type_text("Every\r");
    wait_list(&server, "every repository again", |l| repo(l).is_null());
    // Its box closes on any key, which goes no further.
    client.wait_text("Sidebar: every repository");
    client.type_text("x");
    client.wait_for("the box closed", |s| {
        !s.iter().any(|l| l.contains("Sidebar: every repository"))
    });

    client.type_text(&format!("{ALT}p"));
    client.type_text("Filter the sidebar");
    client.wait_for("the palette naming M-S-R, as it writes M-R", |s| {
        s.iter()
            .any(|l| l.contains("Filter the sidebar to a repos") && l.contains("M-S-R"))
    });
}

// The repository filter's list leads with the repositories filtered to
// most recently: with none filtered to, the one whose tab is in front (the
// first tab's, "home"), then the rest by name, "Every repository" last; with a filter on, "Every
// repository" first and the cursor on the repository before, which Enter
// goes back to. A repository with no tabs left has no row, and the order is
// kept through a restart.
#[test]
fn the_repository_filters_list_leads_with_the_repositories_filtered_to_last() {
    let mut server = TestServer::start();
    settings(&server, json!({}));
    let repos: Vec<_> = ["alpha", "beta", "gamma"]
        .iter()
        .map(|name| {
            let dir = server.dir.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            git(&dir, &["init", "-q"]);
            git(&dir, &["commit", "-q", "--allow-empty", "-m", "start"]);
            dir.canonicalize().unwrap()
        })
        .collect();
    let (alpha, beta, gamma) = (&repos[0], &repos[1], &repos[2]);
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    for (dir, name) in [(alpha, "ant"), (beta, "bee"), (gamma, "cee")] {
        server.ok(&["new-tab", "-d", "-c", &dir.to_string_lossy(), "-n", name]);
        row_of(&mut client, name);
    }
    let side = |l: &Value| l["sessions"][0]["sidebar"].clone();
    let rows = |screen: &[String], texts: &[&str]| -> Vec<usize> {
        texts
            .iter()
            .map(|t| {
                screen
                    .iter()
                    .position(|l| l.contains(t))
                    .unwrap_or_else(|| panic!("no {t:?} in {screen:?}"))
            })
            .collect()
    };
    let rising = |rows: &[usize]| rows.windows(2).all(|w| w[0] < w[1]);

    // No filter: the repository of the tab in front, the rest by name, and
    // "Every repository" last.
    client.type_text(&format!("{ALT}R"));
    client.wait_text("Filter the sidebar to a repository");
    let screen = client.wait_text("gamma  1 tab");
    let order = rows(
        &screen,
        &[
            "home  1 tab",
            "alpha  1 tab",
            "beta  1 tab",
            "gamma  1 tab",
            "Every repository",
        ],
    );
    assert!(rising(&order), "{screen:?}");
    assert!(screen[order[4]].contains("now"), "{screen:?}");
    client.type_text("\x1b");
    client.wait_for("the list closed", |s| {
        !s.iter().any(|l| l.contains("Every repository"))
    });

    server.ok(&["sidebar-repo", &alpha.to_string_lossy()]);
    server.ok(&["sidebar-repo", &beta.to_string_lossy()]);
    wait_list(&server, "filtered to beta after alpha", |l| {
        side(l)["repo_recent"] == json!([beta.to_string_lossy(), alpha.to_string_lossy()])
    });

    // A filter on: "Every repository", then the latest first, and Enter
    // goes back to the repository before.
    client.type_text(&format!("{ALT}R"));
    client.wait_text("Filter the sidebar to a repository");
    let screen = client.wait_text("gamma  1 tab");
    let order = rows(
        &screen,
        &[
            "Every repository",
            "beta  1 tab",
            "alpha  1 tab",
            "home  1 tab",
            "gamma  1 tab",
        ],
    );
    assert!(rising(&order), "{screen:?}");
    assert!(
        screen[order[1]].contains("now") && !screen[order[0]].contains("now"),
        "{screen:?}"
    );
    client.type_text("\r");
    wait_list(&server, "back at alpha", |l| {
        side(l)["repo"] == json!(alpha.to_string_lossy())
            && side(l)["repo_recent"] == json!([alpha.to_string_lossy(), beta.to_string_lossy()])
    });
    client.wait_text("Sidebar: alpha only");
    client.type_text("x");
    client.wait_for("the box closed", |s| {
        !s.iter().any(|l| l.contains("Sidebar: alpha only"))
    });

    // Beta's only tab closed: no row for it.
    let list = server.list();
    let bee = tabs(&list).iter().find(|t| t["name"] == "bee").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["kill-tab", "-t", &bee]);
    wait_list(&server, "bee closed", |l| tabs(l).len() == 3);
    client.type_text(&format!("{ALT}R"));
    client.wait_text("Filter the sidebar to a repository");
    let screen = client.wait_text("gamma  1 tab");
    assert!(!screen.iter().any(|l| l.contains("beta  ")), "{screen:?}");
    client.type_text("gamma\r");
    wait_list(&server, "filtered to gamma", |l| {
        side(l)["repo"] == json!(gamma.to_string_lossy())
    });

    // The order is kept with the layout.
    server.ok(&["save"]);
    drop(client);
    server.restart();
    let after = server.list();
    assert_eq!(
        side(&after)["repo_recent"],
        json!([
            gamma.to_string_lossy(),
            alpha.to_string_lossy(),
            beta.to_string_lossy()
        ]),
        "{after}"
    );
}

// With no repository filtered to, the list ranks the repositories by the
// tab of theirs in front most recently, a step back included: after ant,
// bee and cee, M-{ goes back to bee, and cee's repository, left last, comes
// before ant's, through a restart too.
#[test]
fn the_repository_filters_list_ranks_by_the_tabs_left_through_a_step_back() {
    let mut server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    for (repo, name) in [("alpha", "ant"), ("beta", "bee"), ("gamma", "cee")] {
        let dir = server.dir.path().join(repo);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "start"]);
        server.ok(&["new-tab", "-d", "-c", &dir.to_string_lossy(), "-n", name]);
        row_of(&mut client, name);
    }
    let in_front = |name: &'static str| {
        move |l: &Value| {
            tabs(l)
                .iter()
                .any(|t| t["name"] == name && t["current"] == true)
        }
    };
    for (key, name) in [("2", "ant"), ("3", "bee"), ("4", "cee"), ("{", "bee")] {
        client.type_text(&format!("{ALT}{key}"));
        wait_list(&server, name, in_front(name));
    }

    client.type_text(&format!("{ALT}R"));
    client.wait_text("Filter the sidebar to a repository");
    let in_order = |screen: &[String]| {
        let order: Vec<usize> = ["beta  1 tab", "gamma  1 tab", "alpha  1 tab", "home  1 tab"]
            .iter()
            .map(|t| {
                screen
                    .iter()
                    .position(|l| l.contains(t))
                    .unwrap_or_else(|| panic!("no {t:?} in {screen:?}"))
            })
            .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{screen:?}");
    };
    in_order(&client.wait_text("home  1 tab"));
    client.type_text("\x1b");
    client.wait_for("the list closed", |s| {
        !s.iter().any(|l| l.contains("Every repository"))
    });

    server.ok(&["save"]);
    drop(client);
    server.restart();
    let mut client = server.attach(120, 30);
    row_of(&mut client, "cee");
    client.type_text(&format!("{ALT}R"));
    client.wait_text("Filter the sidebar to a repository");
    in_order(&client.wait_text("home  1 tab"));
}

// A right press on a notification in the Notifications window opens its
// menu, on the window's empty part the window's menu; a right click on a
// tab in the sidebar opens its menu, peeking first.
#[test]
fn a_right_press_in_the_notifications_window_opens_a_menu_and_on_a_tab_peeks() {
    let notes = common::TempDir::new("sbrw");
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
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("n");
    wait_list(&server, "the Notifications window in front", |l| {
        notices_tab(l).is_some_and(|i| tabs(l)[i]["current"] == true)
    });
    let screen = client.wait_text("Written from Fathom into Plan.md");
    let row = screen
        .iter()
        .position(|l| l.contains("▌ Plan"))
        .unwrap_or_else(|| panic!("{}", screen.join("\n")));
    let col = switchboard::ui::draw::width(&screen[row][..screen[row].find("▌ Plan").unwrap()]) + 3;
    let right = |client: &mut TestClient, col: usize, row: usize| {
        client.send(
            format!(
                "\x1b[<2;{};{}M\x1b[<2;{};{}m",
                col + 1,
                row + 1,
                col + 1,
                row + 1
            )
            .as_bytes(),
        );
    };

    right(&mut client, col, row);
    client.wait_text("Open it in Notes");
    client.type_text("\x1b");
    client.wait_for("the menu closed", |s| {
        !s.iter().any(|l| l.contains("Open it in Notes"))
    });
    right(&mut client, col, 25);
    let screen = client.wait_text("Dismiss every notification shown");
    assert!(
        !screen.iter().any(|l| l.contains("Open it in Notes")),
        "{}",
        screen.join("\n")
    );
    client.type_text("\x1b");
    client.wait_for("the window's menu closed", |s| {
        !s.iter()
            .any(|l| l.contains("Dismiss every notification shown"))
    });

    // The tab's row in the sidebar: its menu, peeking first, Enter peeking.
    let row = row_of(&mut client, "  1 ");
    right(&mut client, 6, row);
    client.wait_text("Peek at its screen");
    client.type_text("\r");
    client.wait_text("r capture again");
    // A right press outside the peek closes it, as a left one does.
    right(&mut client, 0, 0);
    client.wait_for("the peek closed", |s| {
        !s.iter().any(|l| l.contains("r capture again"))
    });
}

// Any group folds: `z` on a tab folds its group to its heading and a line
// of its tabs' marks, the cursor onto that line, where Enter unfolds it,
// the cursor onto its first tab and the tab in front kept; a
// click on a heading folds it; the fold outlives an upgrade; and M-1 to a
// tab under it from the sidebar unfolds it again.
#[test]
fn z_or_a_click_on_its_heading_folds_a_group_and_enter_unfolds_it() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-d", "-n", "bee"]);
    row_of(&mut client, "bee");
    let folded = |l: &Value| l["sessions"][0]["sidebar"]["folded"].clone();
    server.ok(&[
        "select-tab",
        "-t",
        tabs(&server.list())[1]["id"].as_str().unwrap(),
    ]);

    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("z");
    wait_list(&server, "Other folded", |l| folded(l) == json!(["other"]));
    let line = row_of(&mut client, "2 tabs");
    assert!(row_of(&mut client, "OTHER 2") < line);
    client.wait_for("bee folded away", |s| {
        !s.iter().any(|l| left(l, 34).contains("bee"))
    });

    // On the folded line, Enter unfolds it, the cursor onto its first tab
    // and bee still in front; Enter again goes into the first tab.
    client.type_text("\r");
    wait_list(&server, "Other unfolded", |l| folded(l) == json!([]));
    row_of(&mut client, "bee");
    let list = server.list();
    assert!(
        tabs(&list)[1]["current"] == true && focused(&list),
        "{list}"
    );
    client.type_text("\r");
    wait_list(&server, "into the first tab", |l| {
        tabs(l)[0]["current"] == true && !focused(l)
    });

    // A click on its heading folds it.
    let heading = row_of(&mut client, "OTHER 2");
    client.send(format!("\x1b[<0;4;{}M\x1b[<0;4;{}m", heading + 1, heading + 1).as_bytes());
    wait_list(&server, "folded by a click", |l| {
        folded(l) == json!(["other"])
    });

    // Kept through an upgrade.
    let (code, out, err) = server.run(&["upgrade"]);
    assert_eq!(code, 0, "{out}{err}\nserver log: {}", server.log());
    wait_list(&server, "the fold kept", |l| folded(l) == json!(["other"]));

    // A digit for a tab under it, from the sidebar, unfolds it.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused again", focused);
    client.type_text("2");
    wait_list(&server, "unfolded for the digit's tab", |l| {
        folded(l) == json!([]) && tabs(l)[1]["current"] == true && focused(l)
    });
}

// Every group's heading is a stop: k from a group's first tab lands on it,
// the tab in front kept; Enter or Space there folds the group, the heading
// still chosen, and again unfolds it; j from a folded heading goes to the
// next group's heading.
#[test]
fn enter_on_a_group_s_heading_folds_and_unfolds_it() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "s-heading"}));
    hook(&server, &pane, "UserPromptSubmit", json!({}));
    server.ok(&["new-tab", "-d", "-n", "bee"]);
    row_of(&mut client, "bee");
    assert!(row_of(&mut client, "WORKING 1") < row_of(&mut client, "OTHER 1"));
    let folded = |l: &Value| l["sessions"][0]["sidebar"]["folded"].clone();
    let has = |l: &Value, stage: &str| {
        folded(l)
            .as_array()
            .is_some_and(|a| a.iter().any(|s| s == stage))
    };

    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("k");
    client.type_text("\r");
    wait_list(&server, "Working folded by Enter", |l| {
        folded(l) == json!(["working"]) && focused(l) && tabs(l)[0]["current"] == true
    });
    client.type_text("\r");
    wait_list(&server, "Working unfolded by Enter", |l| {
        folded(l) == json!([]) && focused(l) && tabs(l)[0]["current"] == true
    });

    // Back onto the heading; Space folds it too.
    client.type_text("k");
    client.type_text(" ");
    wait_list(&server, "Working folded by Space", |l| {
        folded(l) == json!(["working"])
    });

    // j from the folded heading: onto Other's heading, where Enter folds it.
    client.type_text("j");
    client.type_text("\r");
    wait_list(&server, "Other folded from its heading", |l| {
        has(l, "working") && has(l, "other") && tabs(l)[0]["current"] == true
    });
}

// C-u from a group's second tab lands on its first tab, never on the open
// group's heading above it.
#[test]
fn half_a_page_up_passes_an_open_group_s_heading() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("2:second");
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.send(b"\x15");
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true && focused(l)
    });
    // Enter goes into it rather than folding its group.
    client.type_text("\r");
    wait_list(&server, "into the first tab", |l| {
        !focused(l) && l["sessions"][0]["sidebar"]["folded"] == json!([])
    });
}

// With sidebar_reminders off, a tab waiting on a reminder is left out of
// the sidebar and counted on the top line's chip.
#[test]
fn with_sidebar_reminders_off_the_reminders_group_is_a_chip_on_the_top_line() {
    let server = TestServer::start();
    settings(&server, json!({"sidebar_reminders": false}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-d", "-n", "later"]);
    row_of(&mut client, "later");
    let second = tabs(&server.list())[1]["id"].as_str().unwrap().to_string();
    let at = jiff::Timestamp::now().as_second() + 3600;
    server.ok(&["set", "-t", &second, "@remind_at", &at.to_string()]);
    client.wait_for("the chip, and no Reminders group", |s| {
        left(&s[0], 34)
            .split_whitespace()
            .collect::<Vec<_>>()
            .ends_with(&["⏰", "1"])
            && !s.iter().any(|l| left(l, 34).contains("REMINDERS"))
            && !s.iter().any(|l| left(l, 34).contains("later"))
    });
}

// A / search that matches no tab keeps the focus in the sidebar, so
// Backspace can still fix it.
#[test]
fn a_search_matching_nothing_keeps_the_sidebar_focused() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("/zzzq");
    client.wait_for("the search typed", |s| left(&s[1], 34).contains("/zzzq"));
    std::thread::sleep(Duration::from_millis(100) * common::patience());
    assert!(focused(&server.list()), "the search left the sidebar");
    client.type_text("\x7f");
    client.wait_for("Backspace in the search", |s| {
        left(&s[1], 34).contains("/zzz") && !left(&s[1], 34).contains("/zzzq")
    });
}

// m in the sidebar, and the context-menu key (S-F10), open the menu a right
// press on the cursor's row would, beside the row: a tab's with peeking
// first, a folded group's line the sidebar's own with unfolding first, a
// notification's own.
#[test]
fn m_and_shift_f10_in_the_sidebar_open_the_chosen_rows_menu() {
    let notes = common::TempDir::new("sbmk");
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
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    let close = |client: &mut TestClient, text: &str| {
        client.type_text("\x1b");
        client.wait_for("the menu closed", |s| !s.iter().any(|l| l.contains(text)));
    };

    // On a tab: its menu, beside its row, by m and by S-F10.
    let tab = row_of(&mut client, "  1 ");
    for key in ["m", "\x1b[21;2~"] {
        client.type_text(key);
        let screen = client.wait_text("Peek at its screen");
        let at = screen
            .iter()
            .position(|l| l.contains("Peek at its screen"))
            .unwrap();
        assert!(
            at.abs_diff(tab) < 8,
            "the menu by the tab on row {tab}:\n{}",
            screen.join("\n")
        );
        close(&mut client, "Peek at its screen");
    }

    // On a folded group's line: the sidebar's own menu, unfolding first.
    let folded = |l: &Value| l["sessions"][0]["sidebar"]["folded"].clone();
    client.type_text("z");
    wait_list(&server, "Other folded", |l| folded(l) == json!(["other"]));
    client.type_text("m");
    client.wait_text("Unfold Other");
    client.type_text("\r");
    wait_list(&server, "Other unfolded", |l| folded(l) == json!([]));

    // On a notification: its own menu.
    client.type_text("n");
    client.wait_text("NOTIFICATIONS 1");
    client.type_text("m");
    client.wait_text("Open it in Notes");
    close(&mut client, "Open it in Notes");
    assert!(focused(&server.list()), "still in the sidebar");

    // Into the Notifications window: S-F10 there opens the chosen one's.
    client.type_text("\r");
    wait_list(&server, "the focus in the window", |l| !focused(l));
    client.type_text("\x1b[21;2~");
    client.wait_text("Open it in Notes");
}

// A right press in the sidebar opens a menu by it: a notification's own, as
// the Notifications window opens it, and the sidebar's own elsewhere, whose
// "Dismiss every notification shown" empties the notifications; with
// Settings' "Click in menus" off, a right press on a tab peeks as before.
#[test]
fn a_right_press_in_the_sidebar_opens_a_menu_by_it() {
    let notes = common::TempDir::new("sbrm");
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
    let right = |client: &mut TestClient, col: usize, row: usize| {
        client.send(
            format!(
                "\x1b[<2;{};{}M\x1b[<2;{};{}m",
                col + 1,
                row + 1,
                col + 1,
                row + 1
            )
            .as_bytes(),
        );
    };

    let notice = row_of(&mut client, "New note: Plan");
    right(&mut client, 6, notice);
    client.wait_text("Open it in Notes");
    client.type_text("\x1b");
    client.wait_for("the notification's menu closed", |s| {
        !s.iter().any(|l| l.contains("Open it in Notes"))
    });

    // A group's heading: folding it first, then unfolding it.
    let folded = |l: &Value| l["sessions"][0]["sidebar"]["folded"].clone();
    let heading = row_of(&mut client, "OTHER 1");
    right(&mut client, 4, heading);
    client.wait_text("Fold Other");
    client.type_text("\r");
    wait_list(&server, "Other folded", |l| folded(l) == json!(["other"]));
    let heading = row_of(&mut client, "OTHER 1");
    right(&mut client, 4, heading);
    client.wait_text("Unfold Other");
    client.type_text("\r");
    wait_list(&server, "Other unfolded", |l| folded(l) == json!([]));
    client.type_text("q");
    wait_list(&server, "out of the sidebar", |l| !focused(l));

    // A blank row under the tabs: the sidebar's own menu, by the press.
    let tab = row_of(&mut client, "  1 ");
    let screen = client.screen();
    let blank = (tab + 1..screen.len())
        .find(|&r| left(&screen[r], 24).trim().is_empty())
        .unwrap_or_else(|| panic!("{}", screen.join("\n")));
    right(&mut client, 6, blank);
    let screen = client.wait_text("Dismiss every notification shown");
    let at = screen
        .iter()
        .position(|l| l.contains("New tab"))
        .unwrap_or_else(|| panic!("{}", screen.join("\n")));
    assert!(
        at.abs_diff(blank) < 8,
        "the menu by the press on row {blank}:\n{}",
        screen.join("\n")
    );
    client.type_text("G\r");
    client.wait_for("the notifications dismissed", |s| {
        !s.iter().any(|l| left(l, 34).contains("New note: Plan"))
    });
    client.wait_text("Dismissed 1 notifications.");
    client.type_text("\x1b");
    client.wait_for("the message closed", |s| {
        !s.iter().any(|l| l.contains("Dismissed 1 notifications."))
    });

    // With "Click in menus" off, a right press on a tab peeks, once the
    // server's tick has read the settings again (the menu until then).
    settings(&server, json!({"menu_mouse": false}));
    let tab = row_of(&mut client, "  1 ");
    let deadline = common::deadline(Duration::from_secs(10));
    loop {
        right(&mut client, 6, tab);
        let screen = client.wait_for("a peek or the tab's menu", |s| {
            s.iter()
                .any(|l| l.contains("r capture again") || l.contains("Peek at its screen"))
        });
        if screen.iter().any(|l| l.contains("r capture again")) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "still a menu with menu_mouse off:\n{}",
            screen.join("\n")
        );
        client.type_text("\x1b");
        client.wait_for("the menu closed", |s| {
            !s.iter().any(|l| l.contains("Peek at its screen"))
        });
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Five tabs, the first in front, the focus in the sidebar.
fn five_tabs_in_the_sidebar(server: &TestServer) -> TestClient {
    let mut client = server.attach(120, 40);
    client.wait_text("$");
    for n in 2..=5 {
        server.ok(&["new-tab", "-d", "-n", &format!("t{n}")]);
    }
    client.type_text(&format!("{ALT}s"));
    wait_list(server, "the sidebar focused", focused);
    client
}

#[test]
fn v_selects_several_tabs_in_the_sidebar_and_one_menu_closes_them() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = five_tabs_in_the_sidebar(&server);
    // v on a tab, then j and Space twice: three selected.
    client.type_text("j");
    client.type_text("v");
    client.wait_text("1 selected");
    client.type_text("j");
    client.type_text(" ");
    client.wait_text("2 selected");
    client.type_text("j");
    client.type_text(" ");
    client.wait_text("3 selected");
    client.wait_for("three rows ticked", |s| {
        s.iter().filter(|l| left(l, 34).contains('◆')).count() == 3
    });

    // A / filter typed in select mode: Enter keeps it and the selection.
    client.type_text("/t");
    client.type_text("\r");
    client.wait_text("/t ");
    client.wait_text("3 selected");
    client.type_text("\x1b");
    client.wait_for("the filter cleared", |s| {
        !s.iter().any(|l| left(l, 34).contains("/t"))
    });
    client.wait_text("3 selected");

    // Enter: the menu, then its question; no closes none and keeps them.
    client.type_text("\r");
    client.wait_text("Close these tabs");
    client.type_text("\r");
    client.wait_text("Close 3 tabs?");
    client.type_text("n");
    client.wait_for("the question closed", |s| {
        !s.iter().any(|l| l.contains("Close 3 tabs?"))
    });
    assert_eq!(tabs(&server.list()).len(), 5);
    client.wait_text("3 selected");

    // Yes closes the three and leaves the other two.
    client.type_text("m");
    client.wait_text("Close these tabs");
    client.type_text("\r");
    client.wait_text("Close 3 tabs?");
    client.type_text("y");
    client.wait_text("Closed 3 tabs.");
    wait_list(&server, "two tabs left", |l| tabs(l).len() == 2);
    // Select mode and the sidebar left, back in the pane in front.
    wait_list(&server, "back out of the sidebar", |l| !focused(l));
    client.wait_for("select mode left", |s| {
        !s.iter().any(|l| left(l, 34).contains("selected"))
    });
}

#[test]
fn esc_leaves_select_mode_and_a_shift_click_starts_it() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = five_tabs_in_the_sidebar(&server);
    client.type_text("v");
    client.wait_text("1 selected");
    // Esc: out of select mode, nothing selected, the focus still here.
    client.type_text("\x1b");
    client.wait_for("select mode left", |s| {
        !s.iter().any(|l| left(l, 34).contains("selected"))
    });
    assert!(focused(&server.list()));
    assert!(!client.screen().iter().any(|l| left(l, 34).contains('◆')));

    // A Shift-click on a tab's row selects it, a plain click another.
    client.type_text("\x1b");
    wait_list(&server, "back out", |l| !focused(l));
    let row = row_of(&mut client, "  2 ");
    client.send(format!("\x1b[<4;6;{}M\x1b[<4;6;{}m", row + 1, row + 1).as_bytes());
    client.wait_text("1 selected");
    let row = row_of(&mut client, "  3 ");
    client.send(format!("\x1b[<0;6;{}M\x1b[<0;6;{}m", row + 1, row + 1).as_bytes());
    client.wait_text("2 selected");
    assert_eq!(tabs(&server.list()).len(), 5);
}

#[test]
fn with_sidebar_select_off_v_does_nothing() {
    // Off from the start, so v can't come before the settings are read.
    let server = TestServer::start_with_settings(
        r#"{"notifications": false, "tips": false, "sidebar_select": false}"#,
    );
    let mut client = five_tabs_in_the_sidebar(&server);
    client.type_text("v");
    client.type_text("j");
    wait_list(&server, "the second tab in front", |l| in_front(l) != 0);
    assert!(
        !client
            .screen()
            .iter()
            .any(|l| left(l, 34).contains("selected"))
    );
}

#[test]
fn the_line_beside_a_list_cut_short_is_its_scroll_bar_and_a_press_below_the_thumb_pages_down() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = short_screen_of_tabs(&server);
    let first = client.screen()[0].clone();
    let side = first.chars().position(|c| c == '│').unwrap();
    let beside = |s: &[String]| -> Vec<char> {
        s.iter()
            .map(|l| l.chars().nth(side).unwrap_or(' '))
            .collect()
    };
    let screen = client.wait_for("the thumb beside the list", |s| beside(s).contains(&'┃'));
    let column = beside(&screen);
    let last_thumb = column.iter().rposition(|c| *c == '┃').unwrap();
    let below = column
        .iter()
        .enumerate()
        .skip(last_thumb + 1)
        .find(|(_, c)| **c == '│')
        .map(|(y, _)| y)
        .unwrap();
    let (x, y) = (side + 1, below + 1);
    client.send(format!("\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m").as_bytes());
    client.wait_for("tabs above counted", |s| {
        left(&s[1], 34).contains("▲") && left(&s[1], 34).contains("more")
    });
}

#[test]
fn the_pointer_is_a_hand_over_a_tab_s_row_and_an_arrow_over_a_divider() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach_as("xterm-ghostty", 120, 30);
    client.wait_text("$");
    let row = row_of(&mut client, "  1 ") + 1;
    let hand = b"\x1b]22;pointer\x1b\\";
    let count = |client: &TestClient, needle: &[u8]| {
        client
            .raw
            .windows(needle.len())
            .filter(|w| *w == needle)
            .count()
    };
    client.send(format!("\x1b[<35;6;{row}M").as_bytes());
    client.wait_sent("a hand over the tab's row", hand);
    // Moving on within the row sends nothing new; into the pane, the beam.
    client.send(format!("\x1b[<35;8;{row}M\x1b[<35;60;5M").as_bytes());
    client.wait_sent("the beam over the pane", b"\x1b]22;text\x1b\\");
    assert_eq!(count(&client, hand), 1);
    // A pane split beside it: the line between them is a resizing arrow.
    server.ok(&["split", "-R"]);
    let line = client.wait_for("the split", |s| {
        s[5].chars().skip(40).filter(|c| *c == '│').count() == 1
    });
    let col = line[5]
        .chars()
        .enumerate()
        .skip(40)
        .find(|(_, c)| *c == '│')
        .unwrap()
        .0
        + 1;
    client.send(format!("\x1b[<35;{col};6M").as_bytes());
    client.wait_sent("the arrow over the divider", b"\x1b]22;ew-resize\x1b\\");
    // Detaching puts the beam back, whichever build the client is.
    let beam = b"\x1b]22;text\x1b\\";
    let beams = count(&client, beam);
    server.ok(&["detach"]);
    client.wait_exit();
    assert_eq!(count(&client, beam), beams + 1);

    // A terminal that doesn't take OSC 22 is never sent it, nor is one on a
    // server with hover_highlight off.
    let mut plain = server.attach(120, 30);
    plain.wait_text("$");
    plain.send(format!("\x1b[<35;6;{row}M\x1b[<35;60;5M").as_bytes());
    std::thread::sleep(Duration::from_millis(300) * common::patience());
    plain.screen();
    assert_eq!(count(&plain, b"\x1b]22;"), 0);
    let server = TestServer::start_with_settings(
        r#"{"hover_highlight": false, "notifications": false, "tips": false}"#,
    );
    let mut off = server.attach_as("xterm-ghostty", 120, 30);
    off.wait_text("$");
    off.send(format!("\x1b[<35;6;{row}M").as_bytes());
    std::thread::sleep(Duration::from_millis(300) * common::patience());
    off.screen();
    assert_eq!(count(&off, b"\x1b]22;"), 0);
}

/// A press of `button` (0 the left, 2 the right) at a 0-based cell.
fn press(client: &mut TestClient, button: u8, (row, col): (usize, usize)) {
    client.send(
        format!(
            "\x1b[<{button};{};{}M\x1b[<{button};{};{}m",
            col + 1,
            row + 1,
            col + 1,
            row + 1
        )
        .as_bytes(),
    );
}

/// Where a text first shows on the screen: its row and the column it
/// starts at.
fn cell_of(screen: &[String], text: &str) -> Option<(usize, usize)> {
    screen.iter().enumerate().find_map(|(row, line)| {
        let at = line.find(text)?;
        Some((row, switchboard::ui::draw::width(&line[..at])))
    })
}

/// A server with UX 2.0 and three tabs, opened one after another, so the
/// third is in front with the second and the first behind it.
fn three_visited(extra: Value) -> (TestServer, TestClient) {
    let server = TestServer::start();
    let mut all = json!({"ux2": true, "sidebar": true});
    for (k, v) in extra.as_object().unwrap() {
        all[k] = v.clone();
    }
    settings(&server, all);
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    server.ok(&["new-tab", "-n", "third"]);
    wait_list(&server, "the third tab in front", |l| {
        tabs(l).len() == 3 && tabs(l)[2]["current"] == true
    });
    (server, client)
}

#[test]
fn with_ux2_the_top_lines_arrows_step_back_and_forward_and_a_right_click_shows_the_trail() {
    let (server, mut client) = three_visited(json!({}));
    let screen = client.wait_for("the buttons on the top line", |s| {
        s[0].starts_with("  ←   →   ⊞")
    });
    let back = cell_of(&screen, "←").unwrap();
    let forward = cell_of(&screen, "→").unwrap();
    press(&mut client, 0, back);
    wait_list(&server, "back to the second", |l| {
        tabs(l)[1]["current"] == true
    });
    press(&mut client, 0, forward);
    wait_list(&server, "forward to the third", |l| {
        tabs(l)[2]["current"] == true
    });
    // The whole trail by a right click, the first two steps back; a click
    // on it goes straight there.
    press(&mut client, 2, back);
    let screen = client.wait_text("you are here");
    let first = cell_of(&screen, "← 2").unwrap_or_else(|| panic!("{}", screen.join("\n")));
    assert!(screen[first.0].contains("1 "), "{}", screen[first.0]);
    press(&mut client, 0, first);
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    // Now two steps forward, none back.
    press(&mut client, 2, forward);
    // Not the trail's box from before the click, which may not have gone yet.
    let screen = client.wait_for("the trail with two steps forward", |s| {
        cell_of(s, "→ 2").is_some()
    });
    assert!(
        screen.iter().any(|l| l.contains("you are here")),
        "{}",
        screen.join("\n")
    );
    assert!(cell_of(&screen, "← 1").is_none(), "{}", screen.join("\n"));
}

#[test]
fn with_ux2_the_mission_control_button_opens_and_closes_it_and_its_right_click_lists_the_views() {
    let (server, mut client) = three_visited(json!({}));
    let screen = client.wait_for("the buttons on the top line", |s| {
        s[0].starts_with("  ←   →   ⊞")
    });
    let mission = cell_of(&screen, "⊞").unwrap();
    press(&mut client, 0, mission);
    // The sidebar lists its views meanwhile, and its way back to the tabs
    // closes it.
    let screen = client.wait_text("MISSION CONTROL");
    press(&mut client, 0, cell_of(&screen, "‹ Tabs").unwrap());
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    press(&mut client, 2, mission);
    let screen = client.wait_text("Settings");
    for group in ["Work", "Code", "Day", "System"] {
        assert!(
            screen.iter().any(|l| l.contains(group)),
            "{group}:\n{}",
            screen.join("\n")
        );
    }
    let settings = cell_of(&screen, "Settings").unwrap();
    press(&mut client, 0, settings);
    client.wait_text("MISSION CONTROL");
    client.wait_text("INTEGRATIONS");
    // On a view, M-{ steps back to the place before it, the tab it opened
    // over, closing it.
    client.type_text(&format!("{ALT}{{"));
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    wait_list(&server, "back on the third", |l| {
        tabs(l)[2]["current"] == true
    });
}

#[test]
fn with_back_and_forward_off_the_top_line_has_mission_control_alone() {
    let (_server, mut client) = three_visited(json!({"tab_back_forward": false}));
    let screen = client.wait_for("the button on the top line", |s| s[0].starts_with("  ⊞"));
    assert!(!screen[0].contains('←'), "{}", screen[0]);
}

#[test]
fn with_the_sidebar_hidden_the_buttons_start_the_tab_header() {
    let (server, mut client) = three_visited(json!({"sidebar": false}));
    let screen = client.wait_for("the buttons in the header", |s| {
        s[0].starts_with("  ←   →   ⊞")
    });
    press(&mut client, 0, cell_of(&screen, "←").unwrap());
    wait_list(&server, "back to the second", |l| {
        tabs(l)[1]["current"] == true
    });
    let screen = client.wait_for("the buttons in the header", |s| s[0].starts_with("  ←"));
    press(&mut client, 0, cell_of(&screen, "⊞").unwrap());
    client.wait_text("MISSION CONTROL");
}

#[test]
fn with_ux2_the_notifications_window_on_the_trail_is_named_by_the_buttons_and_the_menu() {
    let notes = common::TempDir::new("sbtw");
    std::fs::write(notes.path().join("Plan.md"), "# Plan\n").unwrap();
    let server = TestServer::start_with_env(&[("NOTES_DIR", notes.path().to_str().unwrap())]);
    settings(&server, json!({"ux2": true, "sidebar": true}));
    std::fs::create_dir_all(&server.state).unwrap();
    std::fs::write(
        server.state.join("new-notes.json"),
        json!([{ "path": "Plan.md", "title": "Plan", "source": "Fathom", "at": 1 }]).to_string(),
    )
    .unwrap();
    let mut client = server.attach(140, 36);
    client.wait_text("New note: Plan");
    // Into the Notifications window and back out to the tab.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text("n");
    wait_list(&server, "the Notifications window in front", |l| {
        notices_tab(l).is_some_and(|i| tabs(l)[i]["current"] == true)
    });
    client.type_text(&format!("{ALT}{{"));
    wait_list(&server, "the first tab in front", |l| {
        notices_tab(l).is_some_and(|i| tabs(l)[i]["current"] == false)
    });
    // Forward goes to the window: → isn't faint, and its menu lists it.
    let screen = client.wait_for("the buttons on the top line", |s| {
        s[0].starts_with("  ←   →   ⊞")
    });
    let forward = cell_of(&screen, "→").unwrap();
    client.send(format!("\x1b[<35;{};{}M", forward.1 + 1, forward.0 + 1).as_bytes());
    client.wait_for("the window named on the top line", |s| {
        !s[0].contains("tab")
    });
    press(&mut client, 2, forward);
    let screen = client.wait_text("you are here");
    assert!(cell_of(&screen, "→ 1").is_some(), "{}", screen.join("\n"));
}

#[test]
fn the_trail_menu_keeps_each_rows_steps_in_sight_however_long_a_tabs_name() {
    let (server, mut client) = three_visited(json!({}));
    let id = tabs(&server.list())[1]["id"].as_str().unwrap().to_string();
    server.ok(&["rename-tab", "-t", &id, &"a very long tab name ".repeat(5)]);
    let screen = client.wait_for("the buttons on the top line", |s| {
        s[0].starts_with("  ←   →   ⊞")
    });
    press(&mut client, 2, cell_of(&screen, "←").unwrap());
    let screen = client.wait_text("you are here");
    assert!(cell_of(&screen, "← 1").is_some(), "{}", screen.join("\n"));
    assert!(cell_of(&screen, "← 2").is_some(), "{}", screen.join("\n"));
}

#[test]
fn a_step_chosen_from_the_trail_menu_leaves_the_sidebar() {
    let (server, mut client) = three_visited(json!({}));
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    let screen = client.wait_for("the buttons on the top line", |s| {
        s[0].starts_with("  ←   →   ⊞")
    });
    press(&mut client, 2, cell_of(&screen, "←").unwrap());
    let screen = client.wait_text("you are here");
    press(&mut client, 0, cell_of(&screen, "← 2").unwrap());
    wait_list(&server, "the first tab in front, the sidebar left", |l| {
        tabs(l)[0]["current"] == true && !focused(l)
    });
}

#[test]
fn an_escape_in_a_hook_s_file_name_never_reaches_the_terminal() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "s-1"}));
    // A file named with OSC 52, which would set the clipboard.
    hook(
        &server,
        &pane,
        "PreToolUse",
        json!({"tool_name": "Edit", "tool_input": {"file_path": "/x/a\u{1b}]52;c;aGk=\u{7}.rs"}}),
    );
    row_of(&mut client, "editing a]52;c;aGk=.rs");
    assert!(
        !common::sent(&client.raw, b"\x1b]52;"),
        "the escape reached the terminal"
    );
    assert!(!common::sent(&client.raw, b"aGk=\x07"));
}

#[test]
fn a_tab_whose_command_waits_for_a_slot_says_so_under_working_until_it_holds_one() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    let (pane, pid) = first_pane(&server);
    session_file(&server.home, pid, json!({"sessionId": "s-slot"}));
    hook(&server, &pane, "UserPromptSubmit", json!({}));
    client.wait_for("Working", |s| {
        s.iter().any(|l| left(l, 34).contains("WORKING 1"))
    });
    // The stand-in reports as it waits, with this test's own process as
    // its, so the server's check that it still runs keeps it.
    let me = std::process::id().to_string();
    let report = |args: &[&str]| {
        let (code, _, err) = server.typed_in(&pane, args);
        assert_eq!(code, 0, "{err}");
    };
    report(&["slot-report", "wait", "build", &me, "--", "cargo", "build"]);
    let screen = client.wait_for("the wait in the sidebar", |s| {
        s.iter()
            .any(|l| left(l, 34).contains("waits for a build slot, 1st"))
    });
    assert!(
        screen.iter().any(|l| left(l, 34).contains("WORKING 1")),
        "{}",
        screen.join("\n")
    );
    report(&[
        "slot-report",
        "hold",
        "build",
        &me,
        "0",
        "1",
        "--",
        "cargo",
        "build",
    ]);
    client.wait_for("the wait gone once it holds the slot", |s| {
        !s.iter().any(|l| l.contains("waits for a build slot"))
    });
    report(&["slot-report", "done", &me]);
}

#[test]
fn m_w_in_a_tabs_claude_pane_asks_to_close_the_tab() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("2:second");
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
    let pane = tabs(&server.list())[1]["panes"][0].clone();
    let claude = pane["id"].as_str().unwrap().to_string();
    session_file(
        &server.home,
        pane["pid"].as_u64().unwrap(),
        json!({"sessionId": "s-1"}),
    );
    hook(&server, &claude, "UserPromptSubmit", json!({}));
    // A split beside it closes alone, the tab and its Claude left. M-w
    // waits for the split in front with its shell at its prompt, on a row of
    // its own right of both borders: a login shell's profile can run a
    // command of its own (Ubuntu's runs `locale`) before the prompt, and
    // M-w then asks "Close pane?" rather than closing it.
    client.type_text(&format!("{ALT}L"));
    client.wait_for("a split in the second tab, at its prompt", |s| {
        let list = server.list();
        let panes = tabs(&list)[1]["panes"].as_array().unwrap().clone();
        let split_at_its_prompt = s.iter().any(|row| {
            row.matches('│').count() >= 2 && row.rsplit('│').next().map(str::trim) == Some("$")
        });
        panes.len() == 2
            && panes.iter().any(|p| {
                p["id"] != claude.as_str()
                    && p["active"] == true
                    && matches!(p["command"].as_str(), Some("sh" | "bash" | "dash"))
            })
            && split_at_its_prompt
    });
    // In its Claude pane M-w asks about the tab even with the split there,
    // not only as the tab's one pane.
    let split = tabs(&server.list())[1]["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] != claude.as_str())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["select-pane", "-t", &claude]);
    wait_list(&server, "Claude's pane in front beside the split", |l| {
        tabs(l)[1]["panes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == claude.as_str() && p["active"] == true)
    });
    client.type_text(&format!("{ALT}w"));
    client.wait_text("Close tab 2");
    client.type_text("n");
    client.wait_for("the question gone", |s| {
        !s.iter().any(|l| l.contains("Close tab 2"))
    });
    server.ok(&["select-pane", "-t", &split]);
    wait_list(&server, "the split in front again", |l| {
        tabs(l)[1]["panes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == split.as_str() && p["active"] == true)
    });
    client.type_text(&format!("{ALT}w"));
    wait_list(&server, "the split closed alone", |l| {
        tabs(l).len() == 2 && tabs(l)[1]["panes"].as_array().unwrap().len() == 1
    });
    // In its Claude pane, M-w asks about the tab; no keeps it, yes closes it.
    wait_list(
        &server,
        "the Claude pane in front, known as Claude's",
        |l| {
            let pane = &tabs(l)[1]["panes"][0];
            pane["id"] == claude.as_str() && pane["active"] == true && !pane["agent"].is_null()
        },
    );
    client.type_text(&format!("{ALT}w"));
    client.wait_text("Close tab 2");
    client.type_text("n");
    client.wait_for("the question gone", |s| {
        !s.iter().any(|l| l.contains("Close tab 2"))
    });
    assert_eq!(tabs(&server.list()).len(), 2, "no is no");
    client.type_text(&format!("{ALT}w"));
    client.wait_text("Close tab 2");
    client.type_text("\r");
    wait_list(&server, "the Claude tab closed", |l| tabs(l).len() == 1);
}

#[test]
fn m_w_in_a_tabs_one_pane_without_claude_closes_the_tab() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("2:second");
    client.type_text("sleep 100\r");
    wait_list(&server, "sleep in front", |l| {
        tabs(l)[1]["panes"][0]["command"] == "sleep"
    });
    // Closing the pane alone would only give it a fresh shell, so M-w asks
    // about the tab, as from its Claude pane.
    client.type_text(&format!("{ALT}w"));
    client.wait_text("Close tab 2");
    client.type_text("n");
    client.wait_for("the question gone", |s| {
        !s.iter().any(|l| l.contains("Close tab 2"))
    });
    assert_eq!(tabs(&server.list()).len(), 2, "no is no");
    client.type_text(&format!("{ALT}w"));
    client.wait_text("Close tab 2");
    client.type_text("\r");
    wait_list(&server, "the tab closed", |l| tabs(l).len() == 1);
}

#[test]
fn m_w_closes_the_last_pane_and_its_tab_with_the_setting_off() {
    let server = TestServer::start();
    settings(&server, json!({"close_tab_from_claude": false}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("2:second");
    // Its shell in front, so M-w doesn't ask: a login shell's profile can
    // run a command of its own (Ubuntu's runs `locale`) first.
    wait_list(
        &server,
        "the second tab in front, its shell in front",
        |l| {
            tabs(l)[1]["current"] == true
                && matches!(
                    tabs(l)[1]["panes"][0]["command"].as_str(),
                    Some("sh" | "bash" | "dash")
                )
        },
    );
    client.wait_text("$");
    client.type_text(&format!("{ALT}w"));
    wait_list(&server, "the second tab closed with its pane", |l| {
        tabs(l).len() == 1
    });
}

#[test]
fn m_w_on_a_sidebar_row_asks_to_close_that_tab() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("2:second");
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the sidebar focused", focused);
    client.type_text(&format!("{ALT}w"));
    client.wait_text("Close tab 2");
    client.type_text("\r");
    wait_list(&server, "the second tab closed", |l| tabs(l).len() == 1);
}

// M-O goes to the notifications: the Notifications window in front, from a
// tab and from mission control.
#[test]
fn m_o_goes_to_the_notifications() {
    let notes = common::TempDir::new("sbmo");
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

    client.type_text(&format!("{ALT}O"));
    wait_list(&server, "the Notifications window in front", |l| {
        notices_tab(l).is_some_and(|i| tabs(l)[i]["current"] == true)
    });
    client.wait_text("NOTIFICATIONS 1");

    // Again, it stays there, as M-p's command does.
    client.type_text(&format!("{ALT}O"));
    let list = server.list();
    assert!(
        notices_tab(&list).is_some_and(|i| tabs(&list)[i]["current"] == true),
        "still in front: {list}"
    );

    // k in the sidebar: back up to the tab.
    client.type_text("k");
    wait_list(&server, "the tab in front", |l| {
        tabs(l)[0]["current"] == true
    });

    // From mission control, M-O closes it and goes to the notifications.
    client.type_text(&format!("{ALT} "));
    client.wait_text("MISSION CONTROL");
    client.type_text(&format!("{ALT}O"));
    wait_list(&server, "the window in front from mission control", |l| {
        notices_tab(l).is_some_and(|i| tabs(l)[i]["current"] == true)
    });
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
}

// With the Notifications window off, M-O puts the sidebar's cursor on the
// first notification.
#[test]
fn without_the_window_m_o_moves_the_sidebar_cursor_onto_the_first_notification() {
    let notes = common::TempDir::new("sbmn");
    std::fs::write(notes.path().join("Plan.md"), "# Plan\n").unwrap();
    let server = TestServer::start_with_env_and_settings(
        &[("NOTES_DIR", notes.path().to_str().unwrap())],
        &json!({ "notifications": false, "tips": false, "notice_list": false }).to_string(),
    );
    std::fs::create_dir_all(&server.state).unwrap();
    std::fs::write(
        server.state.join("new-notes.json"),
        json!([{ "path": "Plan.md", "title": "Plan", "source": "Fathom", "at": 1 }]).to_string(),
    )
    .unwrap();
    let mut client = server.attach(140, 36);
    client.wait_text("● New note: Plan");

    client.type_text(&format!("{ALT}O"));
    let list = wait_list(&server, "the sidebar focused", focused);
    assert_eq!(notices_tab(&list), None, "no window opened");
    client.wait_for("the cursor on the notification", |s| {
        s.iter()
            .any(|l| l.contains("▌") && l.contains("New note: Plan"))
    });
}

/// The first session's sidebar's columns, its line beside the panes
/// included.
fn side_columns(server: &TestServer) -> u64 {
    server.list()["sessions"][0]["sidebar"]["columns"]
        .as_u64()
        .unwrap_or(0)
}

/// The first tab's first pane's columns.
fn pane_columns(server: &TestServer) -> u64 {
    tabs(&server.list())[0]["panes"][0]["cols"]
        .as_u64()
        .unwrap_or(0)
}

/// A press on column `from` of row 10 (both from 0), a drag to column `to`
/// and the release there.
fn drag_edge(client: &mut TestClient, from: u64, to: u64) {
    let (from, to) = (from + 1, to + 1);
    client.send(format!("\x1b[<0;{from};11M\x1b[<32;{to};11M\x1b[<0;{to};11m").as_bytes());
}

#[test]
fn the_sidebars_edge_drags_and_the_resize_keys_move_it_kept_through_a_restart() {
    let mut server = TestServer::start_with_settings(
        r#"{"notifications": false, "tips": false, "ux2": true, "auto_restore": true}"#,
    );
    let mut client = server.attach(200, 40);
    client.wait_text("$");
    wait_list(&server, "the sidebar at its default width", |_| {
        side_columns(&server) == 35
    });
    assert_eq!(pane_columns(&server), 165);
    // Its edge is the column after its 34: dragged to column 50, the
    // sidebar is 50 columns and the pane beside it starts after its line.
    drag_edge(&mut client, 34, 50);
    wait_list(&server, "the sidebar dragged to 50 columns", |_| {
        side_columns(&server) == 51
    });
    assert_eq!(pane_columns(&server), 149, "the pane laid out beside it");
    // With the keys in the sidebar, C-M-S-h narrows it by 5 and C-M-S-l
    // widens it; in the pane they leave it alone.
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the focus in the sidebar", focused);
    client.send(b"\x1b[104;8u");
    wait_list(&server, "the sidebar 5 columns narrower", |_| {
        side_columns(&server) == 46
    });
    client.send(b"\x1b[108;8u\x1b[108;8u");
    wait_list(&server, "the sidebar 10 columns wider", |_| {
        side_columns(&server) == 56
    });
    assert!(focused(&server.list()), "the sidebar keeps the keys");
    // No narrower than 24 columns, no wider than half the window.
    drag_edge(&mut client, 55, 3);
    wait_list(&server, "the sidebar at its least", |_| {
        side_columns(&server) == 25
    });
    drag_edge(&mut client, 24, 180);
    wait_list(&server, "the sidebar at half the window", |_| {
        side_columns(&server) == 101
    });
    drag_edge(&mut client, 100, 50);
    wait_list(&server, "the sidebar back at 50 columns", |_| {
        side_columns(&server) == 51
    });
    // Mission control's list of views is drawn in the sidebar's place at
    // that width, and its edge drags the same.
    client.send(b"\x1b");
    client.type_text(&format!("{ALT} "));
    client.wait_text("MISSION CONTROL");
    drag_edge(&mut client, 50, 44);
    client.wait_for("the edge dragged in mission control", |_| {
        side_columns(&server) == 45
    });
    client.type_text(&format!("{ALT} "));
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    assert_eq!(pane_columns(&server), 155, "the tab laid out beside it");
    // Saved with the layout, so a server started again has it.
    server.ok(&["save"]);
    drop(client);
    server.restart();
    let mut client = server.attach(200, 40);
    client.wait_text("$");
    wait_list(&server, "the width kept through a restart", |_| {
        side_columns(&server) == 45
    });
    // "Use the sidebar's default width", on a key of its own, gives it
    // sidebar.json's again.
    switchboard::util::write_atomic(
        server.home.join(".config/switchboard/keys.json"),
        r#"{"root": {"M-z": "sidebar-default-width"}}"#,
    )
    .unwrap();
    let deadline = common::deadline(Duration::from_secs(10));
    while !server.ok(&["list-keys"]).contains("M-z") {
        assert!(Instant::now() < deadline, "keys.json not read");
        std::thread::sleep(Duration::from_millis(50));
    }
    client.type_text(&format!("{ALT}z"));
    wait_list(&server, "the sidebar back at its default width", |_| {
        side_columns(&server) == 35
    });
}

#[test]
fn the_sidebars_own_menu_goes_back_to_the_default_width() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(200, 40);
    client.wait_text("$");
    wait_list(&server, "the sidebar at its default width", |_| {
        side_columns(&server) == 35
    });
    drag_edge(&mut client, 34, 50);
    wait_list(&server, "the sidebar dragged to 50 columns", |_| {
        side_columns(&server) == 51
    });
    // A right click on a blank row of the sidebar opens its own menu.
    client.send(b"\x1b[<2;6;31M\x1b[<2;6;31m");
    let screen = client.wait_text("Use the default width");
    let (y, x) = screen
        .iter()
        .enumerate()
        .find_map(|(y, l)| {
            l.find("Use the default width")
                .map(|i| (y, l[..i].chars().count()))
        })
        .unwrap();
    client.send(format!("\x1b[<0;{};{}M\x1b[<0;{};{}m", x + 2, y + 1, x + 2, y + 1).as_bytes());
    wait_list(&server, "the sidebar back at its default width", |_| {
        side_columns(&server) == 35
    });
}

#[test]
fn with_resizing_off_the_sidebars_edge_takes_no_drag_and_a_kept_width_is_left_alone() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(200, 40);
    client.wait_text("$");
    drag_edge(&mut client, 34, 50);
    wait_list(&server, "the sidebar dragged to 50 columns", |_| {
        side_columns(&server) == 51
    });
    // Off, the sidebar is at sidebar.json's width again and neither a drag
    // nor the keys move it.
    settings(&server, json!({"sidebar_resize": false}));
    wait_list(&server, "the sidebar at its fixed width", |_| {
        side_columns(&server) == 35
    });

    drag_edge(&mut client, 34, 60);
    client.type_text(&format!("{ALT}s"));
    wait_list(&server, "the focus in the sidebar", focused);
    client.send(b"\x1b[108;8u");
    std::thread::sleep(Duration::from_millis(300) * common::patience());
    assert_eq!(side_columns(&server), 35);
    // On again, the width it was given is back.
    settings(&server, json!({}));
    wait_list(&server, "the kept width back", |_| {
        side_columns(&server) == 51
    });
}
