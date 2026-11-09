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

mod common;

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
    let deadline = Instant::now() + Duration::from_secs(10);
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
