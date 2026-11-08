//! Mission control as a person uses it, against a server of the test's own
//! (the tmux version's test/tmux-sidebar.test.js, for its mission client's
//! keys): M-Space opens it over the whole screen, `]` moves to the next
//! view, Enter on a tab's card opens its menu and "Go to tab" goes there,
//! the Settings view changes a setting in the settings file, M-j opens the
//! Work map as a picker of the open tabs, and q closes it.

mod common;

use std::time::{Duration, Instant};

use common::{TestClient, TestServer};
use serde_json::{Value, json};

const ALT: &str = "\x1b";

/// Settings for the tests: no desktop notifications, no tips, no sidebar
/// beside the panes, and only the views a test looks at.
fn settings(server: &TestServer, extra: Value) {
    let dir = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&dir).unwrap();
    let mut all = json!({
        "notifications": false,
        "tips": false,
        "sidebar": false,
        "github": false,
        "linear": false,
        "view_work_map": false,
        "view_calendar": false,
        "view_slack": false,
        "view_closed": false,
        "view_sessions": false,
        "view_resources": false,
        "usage": false,
        "view_keys": false,
        "standup_note": false,
        "meeting_notes": false,
        "new_notes": false,
    });
    for (k, v) in extra.as_object().unwrap() {
        all[k] = v.clone();
    }
    std::fs::write(dir.join("settings.json"), all.to_string()).unwrap();
}

fn tabs(list: &Value) -> Vec<Value> {
    list["sessions"][0]["tabs"].as_array().unwrap().clone()
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

/// A server with two tabs, the second in front, and a client on it.
fn two_tabs(extra: Value) -> (TestServer, TestClient) {
    let server = TestServer::start();
    settings(&server, extra);
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    // The settings taken up: the sidebar gone from beside the panes.
    wait_list(&server, "the settings read", |l| {
        l["sessions"][0]["sidebar"]["columns"] == 0
    });
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("2:second");
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
    (server, client)
}

fn open(client: &mut TestClient) -> Vec<String> {
    client.type_text(&format!("{ALT} "));
    client.wait_text("MISSION CONTROL")
}

/// Takes a menu's option by its label, as the daemon's menus were used:
/// j down to it, Enter. Its options have no letters of their own.
fn pick(client: &mut TestClient, label: &str) {
    for _ in 0..20 {
        let screen = client.wait_text(label);
        if screen.iter().any(|l| l.contains('›') && l.contains(label)) {
            client.type_text("\r");
            return;
        }
        let before = screen.join("\n");
        client.type_text("j");
        client.wait_for("the cursor to move", |s| s.join("\n") != before);
    }
    panic!("{label} not reached");
}

#[test]
fn m_space_opens_it_over_the_screen_and_q_closes_it() {
    let (_server, mut client) = two_tabs(json!({}));
    let screen = open(&mut client);
    assert!(screen[0].contains("MISSION CONTROL"), "{:?}", screen[0]);
    assert!(screen[0].contains(" Tabs 2 "), "{:?}", screen[0]);
    // Each tab a card, in a box for its stage: neither has a Claude.
    assert!(
        screen.iter().any(|l| l.contains("OTHER 2")),
        "{}",
        screen.join("\n")
    );
    assert!(screen.iter().any(|l| l.contains("second")));
    assert!(
        screen.last().unwrap().contains("hjkl move"),
        "the keys at the bottom: {:?}",
        screen.last()
    );
    client.type_text("q");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    // M-Space again opens it, and again closes it.
    open(&mut client);
    client.type_text(&format!("{ALT} "));
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
}

#[test]
fn a_bracket_moves_to_the_next_view_and_round() {
    let (_server, mut client) = two_tabs(json!({}));
    open(&mut client);
    // After the tabs: the notifications, then Settings.
    client.type_text("]");
    client.wait_text("No notifications.");
    client.type_text("]");
    let screen = client.wait_text("INTEGRATIONS");
    assert!(
        screen.last().unwrap().contains("↵ change"),
        "{:?}",
        screen.last()
    );
    client.type_text("[");
    client.wait_text("No notifications.");
    client.type_text("[");
    client.wait_text("OTHER 2");
}

#[test]
fn the_landing_view_opens_with_nothing_landing_saying_so_and_goes_once_left() {
    let (server, mut client) = two_tabs(json!({}));
    // M-p's "Mission control: Landing", by its name in keys.json.
    server.ok(&["press", "mission-landing"]);
    let screen = client.wait_text("Nothing is landing now.");
    assert!(
        screen[0].contains("Landing"),
        "kept while in front: {:?}",
        screen[0]
    );
    // Left for Settings, it goes, nothing landing: back is the
    // notifications.
    client.type_text("]");
    client.wait_text("INTEGRATIONS");
    client.type_text("[");
    let screen = client.wait_text("No notifications.");
    assert!(!screen[0].contains("Landing"), "{:?}", screen[0]);
}

#[test]
fn enter_on_a_tabs_card_and_go_to_tab_goes_there() {
    let (server, mut client) = two_tabs(json!({}));
    open(&mut client);
    // The cursor starts on the first card: tab 1's, whose menu Enter opens.
    client.type_text("\r");
    let screen = client.wait_text("Go to tab 1");
    assert!(screen.iter().any(|l| l.contains("Rename it")));
    assert!(screen.iter().any(|l| l.contains("Close the tab")));
    // Keys typed with the Enter that takes an option go nowhere, rather
    // than into the tab it went to.
    client.type_text("\rxyz");
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !client.screen().iter().any(|l| l.contains("xyz")),
        "{:#?}",
        client.screen()
    );
    // l to the second card, Enter twice goes there. A menu's letters take
    // nothing of their own: g there is no "Go to tab", and j moves.
    open(&mut client);
    client.type_text("l");
    client.type_text("\r");
    client.wait_text("Go to tab 2");
    client.type_text("g");
    client.type_text("j");
    client.wait_for("the cursor on the second option", |s| {
        s.iter().any(|l| l.contains('›') && l.contains("Peek at"))
    });
    assert_eq!(
        tabs(&server.list())[0]["current"],
        true,
        "still on the first"
    );
    client.type_text("k");
    client.wait_for("the cursor back on the first", |s| {
        s.iter()
            .any(|l| l.contains('›') && l.contains("Go to tab 2"))
    });
    client.type_text("\r");
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
}

#[test]
fn the_settings_view_changes_a_setting_in_the_settings_file() {
    let (server, mut client) = two_tabs(json!({}));
    open(&mut client);
    // v and a view's name go there.
    client.type_text("vsett\r");
    let screen = client.wait_text("INTEGRATIONS");
    assert!(
        screen
            .iter()
            .any(|l| l.contains("GitHub") && l.contains(" off ")),
        "{}",
        screen.join("\n")
    );
    // The cursor rests on the first setting, GitHub, not on its category's
    // heading; Enter turns it on.
    client.type_text("\r");
    let file = server.home.join(".config/switchboard/settings.json");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap_or_default();
        if saved.get("github").is_none() && saved["linear"] == false {
            break;
        }
        assert!(Instant::now() < deadline, "not written: {saved}");
        std::thread::sleep(Duration::from_millis(50));
    }
    client.wait_for("GitHub on", |s| {
        s.iter().any(|l| l.contains("GitHub") && l.contains(" on "))
    });
    // / narrows it; Esc clears the filter, then closes it.
    client.type_text("/push notif\r");
    // Push notifications is on the screen before the filter too, so what
    // is waited for is the rest gone.
    client.wait_for("the list narrowed", |s| {
        s.iter().any(|l| l.contains("Push notifications"))
            && !s.iter().any(|l| l.contains("INTEGRATIONS"))
    });
    client.type_text("\x1b");
    client.wait_text("INTEGRATIONS");
    client.type_text("\x1b");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
}

#[test]
fn your_claude_md_row_names_where_the_rules_come_from_and_runs_setup() {
    let (server, mut client) = two_tabs(json!({}));
    std::fs::write(server.home.join("rules.md"), "Be brief.\n").unwrap();
    // The Claude Code setup asks the version of: a stand-in.
    let bin = server.home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(
        bin.join("claude"),
        "#!/bin/sh\necho '2.1.319 (Claude Code)'\n",
    )
    .unwrap();
    std::fs::set_permissions(
        bin.join("claude"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    open(&mut client);
    client.type_text("vsett\r");
    client.wait_text("INTEGRATIONS");
    client.type_text("/own rules, which\r");
    client.wait_text("local file");
    // The cursor rests on the row, not its category's heading.
    client.type_text("\r");
    client.wait_text("Where your CLAUDE.md's rules come from");
    client.type_text("j\r");
    client.wait_text("Your CLAUDE.md: a file");
    client.type_text("~/rules.md\r");
    let file = server.home.join(".config/switchboard/claude.json");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let saved: Value = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        if saved["path"] == "~/rules.md" {
            break;
        }
        assert!(Instant::now() < deadline, "not written: {saved}");
        std::thread::sleep(Duration::from_millis(50));
    }
    // Setup's claude-md part ran: ~/.claude/CLAUDE.md imports the file.
    let claude_md = server.home.join(".claude/CLAUDE.md");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !std::fs::read_to_string(&claude_md).is_ok_and(|t| t.contains("rules.md")) {
        assert!(Instant::now() < deadline, "setup didn't write CLAUDE.md");
        std::thread::sleep(Duration::from_millis(100));
    }
    client.wait_text("Your CLAUDE.md: ~/rules.md");
}

#[test]
fn m_j_picks_an_open_tab_from_the_work_map() {
    let (server, mut client) = two_tabs(json!({}));
    wait_list(&server, "the first tab named", |l| {
        tabs(l)[0]["name"].as_str().is_some_and(|n| !n.is_empty())
    });
    client.type_text(&format!("{ALT}j"));
    let screen = client.wait_text("Work map · open tabs");
    assert!(screen.iter().any(|l| l.contains("tab 1")));
    // Typing filters the rows; Enter opens the chosen one's menu, as the
    // daemon's picker did, its diff and lazygit among what it offers.
    client.type_text("second");
    client.wait_for("one row", |s| !s.iter().any(|l| l.contains("tab 1 ")));
    client.type_text("\r");
    let screen = client.wait_text("Go to tab 2");
    assert!(screen.iter().any(|l| l.contains("Its diff (M-d)")));
    assert!(screen.iter().any(|l| l.contains("lazygit")));
    // Esc closes the menu, not the picker, which types on.
    client.type_text("\x1b");
    client.wait_for("the menu closed", |s| {
        !s.iter().any(|l| l.contains("Go to tab 2"))
    });
    client.type_text("x");
    client.wait_text("secondx");
    client.type_text("\x7f");
    client.wait_for("the filter as it was", |s| {
        !s.iter().any(|l| l.contains("secondx"))
    });
    // Enter twice goes there.
    client.type_text("\r");
    client.wait_text("Go to tab 2");
    client.type_text("\r");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
}

#[test]
fn tab_in_a_picker_goes_round_its_rows_and_stops_on_easy_modes_close_button() {
    let (server, mut client) = two_tabs(json!({}));
    wait_list(&server, "the tabs named", |l| {
        tabs(l)
            .iter()
            .all(|t| t["name"].as_str().is_some_and(|n| !n.is_empty()))
    });
    // Two rows: Tab twice goes round to the first, whose menu Enter opens.
    client.type_text(&format!("{ALT}j"));
    client.wait_text("Work map · open tabs");
    client.type_text("\r");
    let screen = client.wait_text("› Go to tab");
    let first = screen
        .iter()
        .find_map(|l| l.split("› ").nth(1))
        .map(|rest| rest.trim_end_matches([' ', '│']).to_string())
        .unwrap();
    client.type_text("\x1b");
    client.wait_for("the menu closed", |s| !s.iter().any(|l| l.contains("› Go")));
    client.type_text("\t\t\r");
    client.wait_text(&format!("› {first}"));
    client.type_text("\x1b");
    client.wait_for("the menu closed", |s| !s.iter().any(|l| l.contains("› Go")));
    client.type_text("\x1b");
    client.wait_for("the picker closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    // With easy mode's close button, Tab past the last row stops on it,
    // and Enter there closes the picker.
    settings(&server, json!({ "easy_keys": true, "menu_mouse": true }));
    std::thread::sleep(Duration::from_millis(2000));
    client.type_text(&format!("{ALT}j"));
    client.wait_text("Work map · open tabs");
    client.type_text("\t\t\r");
    client.wait_for("the picker closed by its ×", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    assert_eq!(tabs(&server.list())[1]["current"], true, "nowhere gone");
}

#[test]
fn a_reminder_set_from_a_tabs_menu_puts_it_under_reminders() {
    let (server, mut client) = two_tabs(json!({}));
    open(&mut client);
    client.type_text("\r");
    pick(&mut client, "Remind me about it later");
    client.wait_text("In 20 minutes");
    client.type_text("\r");
    client.wait_text("What for?");
    client.type_text("check the build\r");
    let list = wait_list(&server, "the reminder set", |l| {
        tabs(l)[0]["options"]["@remind_at"].is_string()
    });
    assert_eq!(tabs(&list)[0]["options"]["@remind_note"], "check the build");
    let screen = client.wait_text("REMINDERS 1");
    assert!(
        screen.iter().any(|l| l.contains("remind in 2")),
        "{}",
        screen.join("\n")
    );
}

#[test]
fn a_time_typed_into_the_reminder_list_wins_over_the_row_it_resembles() {
    let (server, mut client) = two_tabs(json!({}));
    open(&mut client);
    client.type_text("\r");
    pick(&mut client, "Remind me about it later");
    client.wait_text("Or type a time");
    // "2h" matches "In 20 minutes" and "In 3 hours" by their numbers and
    // letters, but names a time of its own.
    client.type_text("2h\r");
    client.wait_text("What for?");
    client.type_text("\r");
    let list = wait_list(&server, "the reminder set", |l| {
        tabs(l)[0]["options"]["@remind_at"].is_string()
    });
    let at: i64 = tabs(&list)[0]["options"]["@remind_at"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let now = jiff::Timestamp::now().as_second();
    assert!((at - now - 7200).abs() < 60, "{at} is two hours from {now}");
}

#[test]
fn the_calendar_view_draws_today_as_a_day_down_the_hours() {
    let server = TestServer::start();
    settings(&server, json!({"view_calendar": true}));
    let now = jiff::Zoned::now();
    let date = format!("{:04}-{:02}-{:02}", now.year(), now.month(), now.day());
    std::fs::create_dir_all(&server.state).unwrap();
    std::fs::write(
        server.state.join("calendar.json"),
        json!({
            "date": date,
            "updated": now.timestamp().as_millisecond(),
            "events": [
                { "title": "Planning", "start": format!("{date}T09:00:00"), "end": format!("{date}T10:00:00"), "you": "going", "link": "https://calendar.example/1" },
                { "title": "Offsite", "allDay": true, "you": "going" },
            ],
        })
        .to_string(),
    )
    .unwrap();
    let mut client = server.attach(140, 40);
    client.wait_text("$");
    open(&mut client);
    client.type_text("vcalendar\r");
    let screen = client.wait_text("Planning");
    let has = |text: &str| screen.iter().any(|l| l.contains(text));
    assert!(has("all day   Offsite"), "{}", screen.join("\n"));
    assert!(has("   8 AM  ┈"), "the hours down the side");
    assert!(has("   9 AM"));
    assert!(has("Planning"));
    // The chosen event is the first: the all-day one; j goes to the next.
    client.type_text("j\r");
    client.wait_text("Open it in Google Calendar");
}

/// What a line of the screen shows right of the sidebar's edge, if any.
fn pane_text(line: &str) -> &str {
    line.rsplit('│').next().unwrap_or(line).trim()
}

#[test]
fn a_ci_jobs_kept_log_reads_from_its_end_without_its_timestamps() {
    // What the CI view's "Read the failing log" runs beside the pane.
    let server = TestServer::start();
    settings(&server, json!({}));
    let log = server.dir.path().join("77.log");
    let lines: String = (1..=60)
        .map(|n| format!("2026-11-02T10:00:{:02}.0000000Z step {n}\n", n % 60))
        .collect();
    std::fs::write(
        &log,
        lines + "2026-11-02T10:01:00.0000000Z ##[error]it broke\n",
    )
    .unwrap();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    client.type_text(&format!("{} view --log {}\r", common::BIN, log.display()));
    let screen = client.wait_text("Error: it broke");
    assert!(
        screen.iter().any(|l| pane_text(l) == "step 60"),
        "{}",
        screen.join("\n")
    );
    assert!(!screen.iter().any(|l| l.contains("2026-11-02T")));
    assert!(
        !screen.iter().any(|l| pane_text(l) == "step 1"),
        "opened at its end"
    );
    client.type_text("q");
    client.wait_text("$");
}

/// A Claude session file for `pid`, as Claude Code writes one, so the
/// pane's shell stands in for Claude.
fn session_file(home: &std::path::Path, pid: u64, session: &str) {
    let dir = home.join(".claude/sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let file = json!({"pid": pid, "entrypoint": "cli", "sessionId": session});
    std::fs::write(dir.join(format!("{pid}.json")), file.to_string()).unwrap();
}

/// `switchboard hook <event>` in `pane`, as Claude Code runs it.
fn hook(server: &TestServer, pane: &str, event: &str, input: Value) {
    use std::io::Write;
    let mut child = std::process::Command::new(common::BIN)
        .args(["hook", event])
        .env("HOME", &server.home)
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env("SWITCHBOARD", server.socket())
        .env("SWITCHBOARD_PANE", pane)
        .env_remove("SLACK_RELAY_HELPER")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("switchboard runs");
    let _ = child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input.to_string().as_bytes());
    assert!(child.wait().expect("the hook ends").success());
}

/// How many times the screen changes in `span`, looked at every 50ms.
fn changes(client: &mut TestClient, span: Duration) -> usize {
    let until = Instant::now() + span;
    let mut last = client.screen();
    let mut n = 0;
    while Instant::now() < until {
        let now = client.screen();
        if now != last {
            n += 1;
            last = now;
        }
    }
    n
}

#[test]
fn behind_a_box_the_cards_spinners_turn_at_most_every_half_second() {
    let (server, mut client) = two_tabs(json!({}));
    // The sidebar under mission control, whose spinner draws the screen
    // again as often as it turns.
    settings(&server, json!({"sidebar": true}));
    wait_list(&server, "the sidebar back", |l| {
        l["sessions"][0]["sidebar"]["columns"] != 0
    });
    // The first tab's shell stands in for a Claude at work.
    let pane = tabs(&server.list())[0]["panes"][0].clone();
    let id = pane["id"].as_str().unwrap().to_string();
    session_file(&server.home, pane["pid"].as_u64().unwrap(), "s-spin");
    hook(&server, &id, "UserPromptSubmit", json!({"prompt": "go on"}));
    open(&mut client);
    client.wait_text("WORKING 1");
    let free = changes(&mut client, Duration::from_millis(1500));
    assert!(free > 4, "the card's spinner turns: {free} changes");
    // A menu over it: the spinners behind turn at most every 500ms, and
    // the box, merged into the frame, is never sent again under itself, so
    // it can't flicker.
    client.type_text("\r");
    client.wait_text("Go to tab");
    let sent = client.raw.len();
    let boxed = changes(&mut client, Duration::from_millis(1500));
    assert!(boxed <= 4, "{boxed} changes behind the menu in 1.5s");
    let after = String::from_utf8_lossy(&client.raw[sent..]).into_owned();
    assert!(
        !after.contains("Go to tab"),
        "the box sent again: {after:?}"
    );
    assert!(
        after.is_empty() || (after.starts_with("\x1b[?2026h") && after.ends_with("\x1b[?2026l")),
        "each frame as synchronized output"
    );
}

#[test]
fn m_slash_opens_every_key_and_enter_presses_one_in_the_pane_below() {
    let (server, mut client) = two_tabs(json!({ "view_keys": true }));
    client.type_text(&format!("{ALT}/"));
    let screen = client.wait_text("MISSION CONTROL");
    let all = screen.join("\n");
    assert!(all.contains("PANES"), "the workspace's by tag: {all}");
    assert!(all.contains("jk move  ↵ press it"), "{all}");
    // The filter finds a key by what it does, the cursor on it rather than
    // on its group's heading.
    client.type_text("/split pane to the right\r");
    client.wait_for("the list narrowed", |s| {
        s.iter()
            .any(|l| l.contains("M-S-L") && l.contains("Split pane to the right"))
            && !s.iter().any(|l| l.contains("Split pane to the left"))
    });
    // Enter offers to press it, and pressing splits the pane mission
    // control was over.
    client.type_text("\r");
    pick(&mut client, "Press M-S-L");
    wait_list(&server, "the pane split", |l| {
        tabs(l)[1]["panes"].as_array().is_some_and(|p| p.len() == 2)
    });
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
}

#[test]
fn with_the_keys_view_off_m_slash_reaches_the_pane() {
    let (server, mut client) = two_tabs(json!({ "view_keys": false }));
    let pane = tabs(&server.list())[1]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["send-keys", "-t", &pane, "-l", "cat -v"]);
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    client.type_text(&format!("{ALT}/after\r"));
    client.wait_text("^[/after");
    assert!(
        !client
            .screen()
            .iter()
            .any(|l| l.contains("MISSION CONTROL"))
    );
}
