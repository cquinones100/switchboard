//! Mission control as a person uses it, against a server of the test's own
//! (the tmux version's test/tmux-sidebar.test.js, for its mission client's
//! keys): M-Space opens it over the whole screen, `]` moves to the next
//! view, Enter on a tab's card opens its menu and "Go to tab" goes there,
//! the Settings view changes a setting in the settings file, M-j opens the
//! Work map as a picker of the open tabs, and q closes it. With UX 2.0 and
//! many tabs, moving about leaves what it shows standing, while a tab's new
//! output and a tab closing still show. With UX 2.0, the Resources board ends
//! a tab's busy process from its menu after a yes, and a number on a card
//! waiting on you answers its prompt.

use crate::common;

use std::time::{Duration, Instant};

use common::stand_in::Claude;
use common::{TestClient, TestServer};
use serde_json::{Value, json};

const ALT: &str = "\x1b";

/// Settings for the tests: no desktop notifications, no tips, no sidebar
/// beside the panes, and only the views a test looks at.
fn settings(server: &TestServer, extra: Value) {
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
        // The Reminders view too, which a test of reminders turns on.
        "reminders": false,
        "standup_note": false,
        "meeting_notes": false,
        "new_notes": false,
    });
    for (k, v) in extra.as_object().unwrap() {
        all[k] = v.clone();
    }
    common::write_settings(&server.home, all);
}

fn tabs(list: &Value) -> Vec<Value> {
    list["sessions"][0]["tabs"].as_array().unwrap().clone()
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
    // After the tabs: the notifications, Routines, then Settings.
    client.type_text("]");
    client.wait_text("No notifications");
    client.type_text("]");
    client.wait_text("No routines yet");
    client.type_text("]");
    let screen = client.wait_text("INTEGRATIONS");
    assert!(
        screen.last().unwrap().contains("Enter its choices"),
        "{:?}",
        screen.last()
    );
    client.type_text("[");
    client.wait_text("No routines yet");
    client.type_text("[");
    client.wait_text("No notifications");
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
    // Left for Settings, it goes, nothing landing: back is Routines.
    client.type_text("]");
    client.wait_text("INTEGRATIONS");
    client.type_text("[");
    let screen = client.wait_text("No routines yet");
    assert!(!screen[0].contains("Landing"), "{:?}", screen[0]);
}

#[test]
fn with_ux2_moving_through_many_tabs_still_shows_their_output_and_a_tab_closing() {
    let server = TestServer::start();
    settings(&server, json!({"ux2": true}));
    let mut client = server.attach(200, 50);
    client.wait_text("$");
    for n in 2..=6 {
        server.ok(&["new-tab", "-n", &format!("tab{n}")]);
    }
    let list = wait_list(&server, "six tabs", |l| tabs(l).len() == 6);
    let id = |at: usize, key: &str| -> String {
        let tab = &tabs(&list)[at];
        let value = if key == "pane" {
            &tab["panes"][0]["id"]
        } else {
            &tab["id"]
        };
        value.as_str().unwrap().to_string()
    };
    open(&mut client);
    // Three cards along, Enter opens the fourth tab's menu.
    for _ in 0..3 {
        client.type_text("l");
    }
    client.type_text("\r");
    client.wait_text("Go to tab 4");
    client.type_text("\x1b");
    client.wait_for("the menu closed", |s| {
        !s.iter().any(|l| l.contains("Go to tab 4"))
    });
    // Back to the first card: a tab's output shows on its card as it comes.
    client.type_text("g");
    server.ok(&[
        "send-keys",
        "-t",
        &id(0, "pane"),
        "echo shown-$((6*7))",
        "Enter",
    ]);
    client.wait_text("shown-42");
    // A tab closing takes its card away, and the cursor stays on a card.
    server.ok(&["kill-tab", "-t", &id(2, "tab")]);
    client.wait_for("the third tab's card gone", |s| {
        !s.iter().any(|l| l.contains("tab3"))
    });
    // The last card, still tab 6: a tab keeps its number when another closes.
    client.type_text("G");
    client.type_text("\r");
    client.wait_text("Go to tab 6");
}

#[test]
fn with_ux2_a_waiting_cards_menu_answers_its_prompt_and_a_number_does_not() {
    let server = TestServer::start();
    settings(&server, json!({"ux2": true}));
    let mut client = server.attach(160, 50);
    client.wait_text("$");
    let tab = &tabs(&server.list())[0];
    let pane = tab["panes"][0]["id"].as_str().unwrap().to_string();
    let pid = tab["panes"][0]["pid"].as_u64().unwrap();
    // The pane's shell counts as its Claude: a session file names it.
    let sessions = server.home.join(".claude/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join(format!("{pid}.json")),
        json!({"pid": pid, "sessionId": "s-1", "entrypoint": "cli"}).to_string(),
    )
    .unwrap();
    // A permission prompt on its screen, then one key read as it comes.
    let prompt = server.dir.path().join("prompt.txt");
    std::fs::write(
        &prompt,
        "Bash command

  git push origin main

Do you want to proceed?
❯ 1. Yes
  2. Yes, and don't ask again
  3. No
",
    )
    .unwrap();
    server.ok(&[
        "send-keys",
        "-t",
        &pane,
        &format!(
            "clear; cat {}; stty -icanon min 1; x=$(dd bs=1 count=1 2>/dev/null); stty icanon; echo pressed-$x",
            prompt.display()
        ),
        "Enter",
    ]);
    client.wait_text("3. No");
    hook(
        &server,
        &pane,
        "PermissionRequest",
        json!({"tool_name": "Bash", "tool_input": {"command": "git push origin main"}}),
    );
    open(&mut client);
    client.wait_text("NEEDS YOU");
    client.wait_text("BASH COMMAND");
    client.wait_text("Yes, and don't ask again");
    // A number is no answer from mission control: Enter's menu has them.
    client.type_text("2");
    client.type_text("\r");
    // The request whole above its answers.
    client.wait_text("git push origin main");
    pick(&mut client, "Answer: Yes, and don't ask again");
    client.type_text("q");
    client.wait_text("pressed-2");
}

#[test]
fn space_opens_the_chosen_cards_menu_with_nothing_focused() {
    let (_server, mut client) = two_tabs(json!({}));
    open(&mut client);
    client.type_text(" ");
    client.wait_text("Go to tab 1");
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
    // heading; Enter's menu turns it on.
    client.type_text("\r");
    pick(&mut client, "Turn it on");
    let file = server.home.join(".config/switchboard/settings.json");
    let deadline = common::deadline(std::time::Duration::from_secs(10));
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
    Claude::new()
        .version("2.1.319")
        .install(&server.home.join("bin"));
    open(&mut client);
    client.type_text("vsett\r");
    client.wait_text("INTEGRATIONS");
    client.type_text("/own rules, which\r");
    client.wait_text("local file");
    // The cursor rests on the row, not its category's heading.
    client.type_text("\r");
    pick(&mut client, "Change it");
    client.wait_text("Where your CLAUDE.md's rules come from");
    client.type_text("j\r");
    client.wait_text("Your CLAUDE.md: a file");
    client.type_text("~/rules.md\r");
    let file = server.home.join(".config/switchboard/claude.json");
    let deadline = common::deadline(std::time::Duration::from_secs(10));
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
    let deadline = common::deadline(std::time::Duration::from_secs(20));
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
fn with_ux2_the_work_map_s_menus_scope_fold_and_go() {
    let server = TestServer::start();
    settings(&server, json!({ "ux2": true, "view_work_map": true }));
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    wait_list(&server, "two tabs named", |l| {
        tabs(l).len() == 2
            && tabs(l)
                .iter()
                .all(|t| t["name"].as_str().is_some_and(|n| !n.is_empty()))
    });
    open(&mut client);
    client.type_text("vwork\r");
    // The tabs in no repository under a heading of their own, the keys on
    // the footer.
    client.wait_text("▾ Other tabs");
    client.wait_text("m view actions");
    // The view's menu shows only the work with no tab, which leaves
    // nothing; then all.
    client.type_text("m");
    pick(&mut client, "Show only the rows with no tab");
    client.wait_for("nothing without a tab", |s| {
        !s.iter().any(|l| l.contains("Other tabs"))
    });
    client.type_text("m");
    pick(&mut client, "Show every row");
    client.wait_text("▾ Other tabs");
    // Enter on the heading folds it and opens it again; z is no key.
    client.type_text("\r");
    client.wait_text("▸ Other tabs");
    client.type_text("z");
    client.type_text("\r");
    client.wait_text("▾ Other tabs");
    // A row's menu folds its repository too, the cursor on its heading,
    // where Enter opens it again.
    client.type_text("j\r");
    pick(&mut client, "Fold its repository");
    client.wait_text("▸ Other tabs");
    client.type_text("\r");
    client.wait_text("▾ Other tabs");
    // Enter on a row opens its menu, going to its tab first.
    client.type_text("j\r");
    client.wait_text("Go to tab");
    client.type_text("\r");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
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
    std::thread::sleep(Duration::from_millis(2000) * common::patience());
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
    let (server, mut client) = two_tabs(json!({"reminders": true}));
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
    let (server, mut client) = two_tabs(json!({"reminders": true}));
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
    client.wait_text("Open in Google Calendar");
    // The view's own menu goes to today, a day back or a day on.
    client.type_text("\x1b");
    client.wait_for("the menu closed", |s| {
        !s.iter().any(|l| l.contains("Open in Google Calendar"))
    });
    client.type_text("m");
    pick(&mut client, "Go to today");
    client.wait_text("Planning");
}

/// What a line of the screen shows right of the sidebar's edge, if any,
/// the view's scroll bar at its right left out.
fn pane_text(line: &str) -> &str {
    let line = line.trim_end().trim_end_matches(['│', '┃']);
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
fn a_tab_whose_claude_is_behind_a_tool_in_its_pane_s_place_keeps_its_stage() {
    let (server, mut client) = two_tabs(json!({}));
    common::stand_in::lazygit(&server.home.join("bin"));
    // The first tab's shell stands in for a Claude, lazygit in its place.
    let tab = tabs(&server.list())[0].clone();
    let id = tab["panes"][0]["id"].as_str().unwrap().to_string();
    session_file(
        &server.home,
        tab["panes"][0]["pid"].as_u64().unwrap(),
        "s-parked",
    );
    server.ok(&["select-tab", "-t", tab["id"].as_str().unwrap()]);
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    server.ok(&["press", "lazygit"]);
    wait_list(&server, "lazygit in the pane's place", |l| {
        tabs(l)[0]["panes"][0]["options"]["@in_place"] == "lazygit"
    });
    hook(&server, &id, "UserPromptSubmit", json!({"prompt": "go on"}));
    open(&mut client);
    // Its card under Working, the other tab alone under Other.
    let screen = client.wait_text("WORKING 1");
    assert!(
        screen.iter().any(|l| l.contains("OTHER 1")),
        "{}",
        screen.join("\n")
    );
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
    assert!(
        all.contains("jk move  Enter actions  m view actions"),
        "{all}"
    );
    // The filter finds a key by what it does, the cursor on it rather than
    // on its group's heading.
    client.type_text("/split pane to the right\r");
    client.wait_for("the list narrowed", |s| {
        s.iter()
            .any(|l| l.contains("M-S-L") && l.contains("Split pane to the right"))
            && !s.iter().any(|l| l.contains("Split pane to the left"))
    });
    // Enter opens its menu, pressing it first, so Enter twice splits the
    // pane mission control was over.
    client.type_text("\r");
    client.wait_text("Press it here");
    client.type_text("\r");
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

// A Slack message's notification has the Slack view's menu: the message
// whole above the choices, since the row cuts it, and its dismissing worded
// as there.
#[test]
fn a_slack_notifications_menu_is_the_slack_views_with_the_message_whole() {
    let server = TestServer::start();
    std::fs::create_dir_all(&server.state).unwrap();
    let long = "the end of a first line too long for its row ".repeat(4);
    // Sent now: a dismissal is kept two days past its message.
    let ts = format!("{}.000100", switchboard::util::now_ms() / 1000);
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": 1, "items": [{
            "kind": "dm", "channel": "D1", "from": "Ada", "where": "Ada",
            "text": format!("{long}cut here\nits second line\nits third line"),
            "ts": ts, "link": "https://slack.example/m",
        }] })
        .to_string(),
    )
    .unwrap();
    settings(&server, json!({ "view_slack": true }));
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    open(&mut client);
    client.type_text("]");
    client.wait_text("DM Ada");
    // Enter opens a notification's menu.
    client.type_text("j");
    client.type_text("\r");
    let screen = client.wait_text("Dismiss it, until its thread gets a reply");
    let all = screen.join("\n");
    let row = |text: &str| {
        screen
            .iter()
            .position(|l| l.contains(text))
            .unwrap_or_else(|| panic!("{text} isn't shown: {all}"))
    };
    let choices = row("Open it in Slack");
    assert!(row("cut here") < choices, "its first line whole: {all}");
    assert!(row("its second line") < row("its third line"), "{all}");
    assert!(row("its third line") < choices, "above the choices: {all}");
    assert!(
        row("Open a tab for this thread with Claude") < row("Dismiss it, until"),
        "{all}"
    );
    assert!(!all.contains("Dismiss every message shown"), "{all}");
    assert!(
        !all.replace("Dismiss it, until", "").contains("Dismiss it"),
        "one way to dismiss it: {all}"
    );
    // Dismissed as the Slack view dismisses it: until its thread gets more.
    pick(&mut client, "Dismiss it, until its thread gets a reply");
    // Kept in the history, out of the sidebar.
    client.wait_text("dismissed");
    let gone = std::fs::read_to_string(server.state.join("slack-dismissed.json")).unwrap();
    assert!(gone.contains(&format!("D1:{ts}")), "{gone}");
}

#[test]
fn the_notifications_views_menu_dismisses_every_notification_shown() {
    let notes = common::TempDir::new("sbda");
    std::fs::write(notes.path().join("Plan.md"), "# Plan\n").unwrap();
    let server = TestServer::start_with_env(&[("NOTES_DIR", notes.path().to_str().unwrap())]);
    std::fs::create_dir_all(&server.state).unwrap();
    let ts = format!("{}.000100", switchboard::util::now_ms() / 1000);
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": 1, "items": [{
            "kind": "dm", "channel": "D1", "from": "Ada", "where": "Ada",
            "text": "a question", "ts": ts, "link": "https://slack.example/m",
        }] })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        server.state.join("new-notes.json"),
        json!([{ "path": "Plan.md", "title": "Plan", "source": "Fathom",
            "at": switchboard::util::now_ms() }])
        .to_string(),
    )
    .unwrap();
    settings(&server, json!({ "view_slack": true, "new_notes": true }));
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    open(&mut client);
    client.type_text("]");
    client.wait_text("DM Ada");
    client.wait_text("Plan");
    client.type_text("j");
    client.type_text("m");
    pick(&mut client, "Dismiss every notification shown");
    client.wait_text("Dismissed 2 notifications.");
    let gone = std::fs::read_to_string(server.state.join("slack-dismissed.json")).unwrap();
    assert!(gone.contains(&format!("D1:{ts}")), "{gone}");
    let left = std::fs::read_to_string(server.state.join("new-notes.json")).unwrap();
    assert!(!left.contains("Plan.md"), "{left}");
}

// With UX 2.0 the Slack view is board 21: the chosen message's thread,
// read through the connector once when it isn't kept, beside the list.
#[test]
fn choosing_a_slack_message_reads_its_thread_once_and_draws_it_beside() {
    let server = TestServer::start();
    std::fs::create_dir_all(&server.state).unwrap();
    let secs = switchboard::util::now_ms() / 1000;
    let first = format!("{}.000100", secs - 600);
    let reply = format!("{}.000100", secs - 60);
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": secs * 1000, "items": [{
            "kind": "thread", "channel": "C1", "from": "Lin", "user": "ULIN",
            "where": "#dev", "text": "an answer", "ts": reply, "thread": first,
            "link": "https://slack.example/m",
        }] })
        .to_string(),
    )
    .unwrap();
    let thread = format!(
        "=== THREAD PARENT MESSAGE ===\nFrom: Me (UME)\nMessage TS: {first}\nmy first question\n\n=== THREAD REPLIES (1 total) ===\n\n--- Reply 1 of 1 ---\nFrom: Lin (ULIN)\nMessage TS: {reply}\nan answer\n"
    );
    let read = json!({ "type": "assistant", "message": { "content": [{
        "type": "tool_use", "id": "t1", "name": "mcp__claude_ai_Slack__slack_read_thread", "input": {},
    }] } });
    let result = json!({ "type": "user", "message": { "content": [{
        "type": "tool_result", "tool_use_id": "t1",
        "content": [{ "type": "text", "text": json!({ "messages": thread }).to_string() }],
    }] } });
    let calls = server.home.join("thread-reads");
    Claude::new()
        .answer(&format!(
            "echo read >> {}\nprintf '%s\\n' {} {}",
            common::stand_in::sh_quote(&calls.to_string_lossy()),
            common::stand_in::sh_quote(&read.to_string()),
            common::stand_in::sh_quote(&result.to_string()),
        ))
        .install(&server.home.join("bin"));
    settings(
        &server,
        json!({ "view_slack": true, "ux2": true, "slack_threads": true, "slack_background": false, "photos": false }),
    );
    let mut client = server.attach(180, 40);
    client.wait_text("$");
    open(&mut client);
    client.type_text("vslack\r");
    let screen = client.wait_text("my first question");
    let all = screen.join("\n");
    assert!(all.contains("YOUR THREADS"), "{all}");
    assert!(all.contains("thread · 1 reply"), "{all}");
    let kept = std::fs::read_to_string(server.state.join("slack-threads.json")).unwrap();
    assert!(kept.contains(&format!("C1:{first}")), "{kept}");
    // Moving and coming back reads nothing again: it is kept.
    client.type_text("k");
    client.type_text("j");
    client.wait_text("my first question");
    let reads = std::fs::read_to_string(&calls).unwrap();
    assert_eq!(reads.lines().count(), 1, "read once");
}

// A notification no view lists as an item is shown whole above its choices
// in the Notifications view, as the Notifications window shows it: a landing
// made without asking says what it was, then leads to its tab.
#[test]
fn a_notification_no_view_lists_is_whole_above_its_choices() {
    let server = TestServer::start();
    settings(&server, json!({ "autopilot_landing_notices": true }));
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let said = std::process::Command::new(common::BIN)
        .args(["land-lock", "autolanded", "abc1234", "Make", "the", "thing"])
        .current_dir(server.dir.path())
        .env("HOME", &server.home)
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env("SWITCHBOARD", server.socket())
        .env("SWITCHBOARD_PANE", &pane)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        said.status.success(),
        "{}",
        String::from_utf8_lossy(&said.stderr)
    );
    open(&mut client);
    client.type_text("]");
    client.wait_text("Landed abc1234: Make the thing");
    client.type_text("j");
    client.type_text("\r");
    let screen = client.wait_text("Dismiss it");
    let all = screen.join("\n");
    let row = |text: &str| {
        screen
            .iter()
            .position(|l| l.contains(text))
            .unwrap_or_else(|| panic!("{text} isn't shown: {all}"))
    };
    assert!(
        row("Landed on the default branch without asking") < row("Go to its tab"),
        "what it was above its choices: {all}"
    );
    assert!(row("Go to its tab") < row("Dismiss it"), "{all}");
    pick(&mut client, "Dismiss it");
    client.wait_text("dismissed");
}

// The Notifications view keeps every notification of the last two weeks by
// day: the days before yesterday past the first screenful fold until their
// menu's "Show them", and the view's menu narrows it to a kind.
#[test]
fn the_notifications_history_folds_old_days_and_its_menu_narrows_it_to_a_kind() {
    let mut server = TestServer::start();
    settings(&server, json!({}));
    let now = switchboard::util::now_ms();
    let entry = |source: &str, id: &str, title: &str, at: i64| {
        json!({ "source": source, "id": id, "at": at, "sidebar": "older",
            "item": { "id": id, "title": title } })
    };
    let dir = server.state.join("notifications");
    std::fs::create_dir_all(&dir).unwrap();
    let day_of = |ms: i64| switchboard::sidebar::history::day_of(ms, &jiff::tz::TimeZone::system());
    let today: Vec<Value> = (0..30)
        .map(|n| entry("slack", &format!("s{n}"), &format!("Slack {n}"), now - n))
        .collect();
    std::fs::write(
        dir.join(format!("{}.json", day_of(now))),
        Value::from(today).to_string(),
    )
    .unwrap();
    let old = now - 4 * 86_400_000;
    std::fs::write(
        dir.join(format!("{}.json", day_of(old))),
        json!([entry("inbox", "i1", "Old inbox one", old)]).to_string(),
    )
    .unwrap();
    server.restart();
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    open(&mut client);
    client.type_text("]");
    client.wait_text("Slack 0");
    client.type_text("G");
    client.wait_text("1 more, folded");
    assert!(!client.screen().join("\n").contains("Old inbox one"));
    // A folded day is an item: its menu shows it.
    client.type_text("\r");
    pick(&mut client, "Show them");
    client.wait_text("Old inbox one");
    // The PRs, of which there are none, then the Inbox, from `m`'s menu;
    // a number is no key of the view's.
    client.type_text("3");
    client.type_text("m");
    client.wait_text("Dismiss every notification shown");
    pick(&mut client, "Show only PRs");
    client.wait_text("No notifications in the last two weeks.");
    client.type_text("m");
    pick(&mut client, "Show only Inbox");
    let screen = client.wait_text("Old inbox one");
    assert!(
        !screen.join("\n").contains("Slack 0"),
        "{}",
        screen.join("\n")
    );
}

#[test]
fn with_ux2_resources_ends_a_tabs_busy_process_from_its_menu_after_a_yes() {
    let server = TestServer::start();
    settings(&server, json!({"view_resources": true, "ux2": true}));
    let mut client = server.attach(150, 36);
    client.wait_text("$");
    // A process pegging a core in the tab.
    client.type_text("yes > /dev/null\r");
    open(&mut client);
    client.type_text("vresources\r");
    // The tab's row, chosen, with its busiest process under it.
    client.wait_text("CPU, LAST 2 MINUTES");
    client.wait_for("the busy process read", |s| {
        s.iter().any(|l| l.contains("yes ") && l.contains('%'))
    });
    client.type_text("\r");
    client.wait_text("Go to tab 1");
    pick(&mut client, "End yes");
    client.wait_text("End yes in tab 1?");
    client.type_text("\r");
    client.wait_text("Ended yes.");
    // The shell has its prompt back.
    let deadline = common::deadline(Duration::from_secs(10));
    loop {
        let pids: Vec<u32> = server.list()["sessions"][0]["tabs"][0]["panes"][0]["pid"]
            .as_u64()
            .map(|p| vec![p as u32])
            .unwrap_or_default();
        let still = pids.iter().any(|&shell| {
            switchboard::mux::procinfo::children_of(shell)
                .iter()
                .any(|&c| switchboard::mux::procinfo::command_of(c).as_deref() == Some("yes"))
        });
        if !still {
            break;
        }
        assert!(Instant::now() < deadline, "yes still runs");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_key_found_with_a_question_mark_is_not_pressed_and_enter_shows_it_in_the_list() {
    let (server, mut client) = two_tabs(json!({"view_keys": true}));
    client.type_text(&format!("{ALT}/"));
    client.wait_text("PANES");
    // The view's menu looks a key up without pressing it: j is the
    // sidebar's.
    client.type_text("m");
    pick(&mut client, "Find a key by pressing it");
    client.wait_text("Find a key");
    client.type_text("j");
    client.wait_text("Sidebar: ");
    // M-w, which closes mission control elsewhere, is looked up here.
    client.type_text(&format!("{ALT}w"));
    client.wait_text("Close the pane");
    client.type_text(&format!("{ALT}L"));
    let screen = client.wait_text("Before it, M-w");
    assert!(
        screen.iter().any(|l| l.contains("Split pane to the right")),
        "{}",
        screen.join("\n")
    );
    assert!(
        screen.iter().any(|l| l.contains("Before it, M-w")),
        "{}",
        screen.join("\n")
    );
    // Enter shows it in the list, where Enter's menu presses it, splitting
    // the pane. Nothing was pressed while the box was up.
    let panes = |server: &TestServer| {
        tabs(&server.list())[1]["panes"]
            .as_array()
            .map_or(0, Vec::len)
    };
    assert_eq!(panes(&server), 1);
    client.type_text("\r");
    client.wait_for("the box closed", |s| {
        !s.iter().any(|l| l.contains("Find a key"))
    });
    client.type_text("\r");
    pick(&mut client, "Press it here");
    wait_list(&server, "the pane split", |l| {
        tabs(l)[1]["panes"].as_array().is_some_and(|p| p.len() == 2)
    });
}

/// With UX 2.0 the Inbox is a board: each notification by who acted, the
/// chosen one in its ticket beside the list with its own comment marked
/// new. Enter offers the Tickets board's menu without "Mark it read",
/// marking it read in Linear as the menu closes, but for its last choice,
/// which takes it out of the view and leaves it unread there; `o` and `x`
/// are no keys of the view's.
#[test]
fn with_ux2_the_inbox_shows_each_in_its_ticket_and_its_menu_acts_on_it() {
    use common::http::{Fake, Reply};
    let at = |minutes: i64| {
        (jiff::Timestamp::now() - jiff::SignedDuration::from_mins(minutes)).to_string()
    };
    let (ben, ana) = (at(10), at(20));
    // What Linear was told is read, which it then leaves out.
    let read = std::sync::Mutex::new(Vec::<String>::new());
    let linear = Fake::linear("test-key", move |request| {
        let query = request.query();
        let answer = if query.contains("notificationUpdate") {
            let id = request.json()["variables"]["id"]
                .as_str()
                .unwrap_or("")
                .to_string();
            read.lock().unwrap().push(id);
            json!({ "data": { "notificationUpdate": { "success": true } } })
        } else if query.contains("notifications(") {
            let issue = |n: u32, title: &str| json!({ "identifier": format!("ABC-{n}"), "title": title, "url": format!("https://linear.app/acme/issue/ABC-{n}") });
            json!({ "data": { "notifications": { "nodes": [
                { "id": "n1", "type": "issueNewComment", "createdAt": ben, "actor": { "id": "u-ben", "name": "Ben" },
                  "issue": issue(7, "Hold the landing lock"),
                  "comment": { "id": "c-2", "body": "the new one", "url": "https://linear.app/acme/issue/ABC-7#c-2" } },
                { "id": "n2", "type": "issueAssignedToYou", "createdAt": ana, "actor": { "id": "u-ana", "name": "Ana" },
                  "readAt": read.lock().unwrap().contains(&"n2".to_string()).then(|| ana.clone()),
                  "issue": issue(8, "Count the requests"), "comment": null },
            ] } } })
        } else if query.contains("issue(id") {
            let id = request.json()["variables"]["id"]
                .as_str()
                .unwrap_or("")
                .to_string();
            json!({ "data": { "issue": {
                "identifier": id, "title": "A ticket", "url": "https://linear.app/acme/issue/x",
                "updatedAt": jiff::Timestamp::now().to_string(), "state": { "name": "In Progress", "type": "started" },
                "priorityLabel": "High", "description": "What it is about.",
                "comments": { "nodes": [
                    { "id": "c-1", "body": "an earlier thought", "createdAt": at(30), "user": { "id": "u-kai", "name": "Kai" } },
                    { "id": "c-2", "body": "the new one", "createdAt": at(10), "user": { "id": "u-ben", "name": "Ben" } },
                ] }
            } } })
        } else {
            return None;
        };
        Some(Reply::json(200, answer))
    });
    let server = TestServer::start_with_env(&[
        ("LINEAR_API_KEY", "test-key"),
        ("SWITCHBOARD_LINEAR_API", &linear.graphql_url()),
    ]);
    settings(
        &server,
        json!({"ux2": true, "linear": true, "view_inbox": true, "photos": false}),
    );
    let mut client = server.attach(170, 40);
    client.wait_text("$");
    open(&mut client);
    client.type_text("vinbox\r");
    client.wait_text("Ben commented");
    client.wait_text("an earlier thought");
    client.wait_text("Ben  10m ago");
    client.type_text("ox");
    client.type_text("\r");
    pick(&mut client, "Dismiss it here: it stays unread in Linear");
    client.wait_for("ABC-7 out of the view", |s| {
        !s.join("\n").contains("Ben commented") && s.join("\n").contains("assigned to you by Ana")
    });
    client.type_text("\r");
    let menu = client.wait_text("Start work on it in a new tab");
    assert!(
        !menu.join("\n").contains("Mark it read"),
        "{}",
        menu.join("\n")
    );
    client.type_text("\x1b");
    client.wait_for("ABC-8 marked read and out of the view", |s| {
        !s.join("\n").contains("assigned to you by Ana")
    });
    assert!(
        linear.seen().iter().any(
            |r| r.query().contains("notificationUpdate") && r.json()["variables"]["id"] == "n2"
        ),
        "marked read in Linear"
    );
    assert!(
        !linear.seen().iter().any(
            |r| r.query().contains("notificationUpdate") && r.json()["variables"]["id"] == "n1"
        ),
        "a dismissed one stays unread in Linear"
    );
}

/// A server with UX 2.0 on and two tabs, the second in front, and a
/// client on it.
fn two_tabs_ux2() -> (TestServer, TestClient) {
    let server = TestServer::start();
    settings(&server, json!({"ux2": true, "reminders": true}));
    let mut client = server.attach(160, 40);
    client.wait_text("$");
    // UX 2.0 taken up: each tab has a header.
    client.wait_text("M-c Claude");
    server.ok(&["new-tab", "-n", "second"]);
    wait_list(&server, "the second tab in front", |l| {
        tabs(l).len() == 2 && tabs(l)[1]["current"] == true
    });
    client.wait_text("second");
    (server, client)
}

/// The first tab's id, for `set -t`.
fn first_tab_id(server: &TestServer) -> String {
    tabs(&server.list())[0]["id"].as_str().unwrap().to_string()
}

fn remind_at(list: &Value, tab: usize) -> Option<i64> {
    tabs(list)[tab]["options"]["@remind_at"]
        .as_str()
        .and_then(|v| v.parse().ok())
}

/// The box saying what was done (`Server::say`, mission control being
/// closed) put away.
fn said(client: &mut TestClient, words: &str) {
    client.wait_text(words);
    client.type_text("\x1b");
    client.wait_for("the message gone", |s| !s.iter().any(|l| l.contains(words)));
}

/// M-p's "Remind me about this tab", which with UX 2.0 opens the box.
fn remind_box(client: &mut TestClient, title: &str) {
    client.type_text(&format!("{ALT}p"));
    client.wait_text("Actions");
    client.type_text("remind me about this");
    client.wait_text("Remind me about this tab later");
    client.type_text("\r");
    client.wait_text(title);
}

#[test]
fn with_ux2_the_reminder_box_sets_a_time_offered_a_date_typed_and_clears_it() {
    let (server, mut client) = two_tabs_ux2();
    remind_box(&mut client, "Remind me about 2 second");
    client.wait_text("In 3 hours");
    // ↓ ↓ to "In 3 hours", Tab to What for, a note, Enter.
    client.send(b"\x1b[B");
    client.send(b"\x1b[B");
    client.type_text("\t");
    client.type_text("the handoff\r");
    let list = wait_list(&server, "the reminder set", |l| remind_at(l, 1).is_some());
    let now = jiff::Timestamp::now().as_second();
    let at = remind_at(&list, 1).unwrap();
    assert!(
        (at - now - 3 * 3600).abs() < 60,
        "{at} is three hours from {now}"
    );
    assert_eq!(tabs(&list)[1]["options"]["@remind_note"], "the handoff");
    said(&mut client, "Reminder set for 2 second");

    remind_box(&mut client, "Change the reminder for 2 second");
    client.wait_text("Set now:");
    client.type_text("nov 20 3pm");
    client.wait_text("✓ ");
    client.type_text("\r");
    let local = jiff::Zoned::now();
    let expected = mission_when("nov 20 3pm", &local);
    let list = wait_list(&server, "the date set", |l| {
        remind_at(l, 1) == Some(expected)
    });
    assert_eq!(tabs(&list)[1]["options"]["@remind_note"], "the handoff");
    said(&mut client, "Reminder set for 2 second");

    remind_box(&mut client, "Change the reminder for 2 second");
    client.type_text(&format!("{ALT}x"));
    let list = wait_list(&server, "the reminder cleared", |l| {
        remind_at(l, 1).is_none()
    });
    assert!(tabs(&list)[1]["options"]["@remind_note"].is_null());
}

/// When `text` names, as the server reads it.
fn mission_when(text: &str, now: &jiff::Zoned) -> i64 {
    switchboard::mission::remind::when(text, now).unwrap()
}

#[test]
fn with_ux2_the_reminders_view_moves_its_day_and_month_and_its_menu_goes() {
    let (server, mut client) = two_tabs_ux2();
    let id = first_tab_id(&server);
    let soon = (jiff::Timestamp::now().as_second() + 120).to_string();
    server.ok(&["set", "-t", &id, "@remind_at", &soon]);
    server.ok(&["set", "-t", &id, "@remind_note", "look at the build"]);
    open(&mut client);
    client.type_text("vreminders\r");
    client.wait_text("1 reminder");
    client.wait_text("look at the build");
    let today = jiff::Zoned::now().date();
    let title = |d: jiff::civil::Date| {
        format!(
            "{:?}, {} {}",
            d.weekday(),
            switchboard::mission::remind::MONTHS[(d.month() - 1) as usize],
            d.day()
        )
    };
    let month = |d: jiff::civil::Date| {
        format!(
            "{} {}",
            switchboard::mission::remind::MONTHS[(d.month() - 1) as usize].to_uppercase(),
            d.year()
        )
    };
    use switchboard::mission::reminders::step;
    client.type_text("l");
    client.wait_text(&title(step(today, 1, 0)));
    client.type_text("j");
    client.wait_text(&title(step(today, 8, 0)));
    client.type_text("k");
    client.type_text("h");
    client.wait_text(&title(today));
    client.type_text("L");
    client.wait_text(&month(step(today, 0, 1)));
    client.type_text("H");
    client.wait_text(&month(today));
    // t is no key: the view's menu goes back to today.
    client.type_text("L");
    client.type_text("t");
    client.type_text("m");
    pick(&mut client, "Go to today");
    client.wait_text(&title(today));
    client.wait_text("look at the build");
    // x and g are no keys of the view's: Enter's menu is its tab's, going
    // there first.
    client.type_text("xg");
    client.type_text("\r");
    client.wait_text("Change or clear its reminder");
    client.type_text("\r");
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    assert!(remind_at(&server.list(), 0).is_some(), "x cleared nothing");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
}

// `m` opens the view's own menu in every view, ending with mission
// control's: several views at once, and this view's settings, which shows
// Settings filtered by the view's name.
#[test]
fn m_opens_the_views_menu_ending_with_its_settings() {
    let (_server, mut client) = two_tabs_ux2();
    open(&mut client);
    client.type_text("m");
    let screen = client.wait_text("This view's settings");
    let at = |text: &str| {
        screen
            .iter()
            .position(|l| l.contains(text))
            .unwrap_or_else(|| panic!("{text}: {}", screen.join("\n")))
    };
    assert!(at("Several views at once") < at("This view's settings"));
    pick(&mut client, "This view's settings");
    client.wait_text("/Tabs");
}

#[test]
fn the_view_menu_picks_views_to_show_at_once_and_esc_goes_back_to_one() {
    let server = TestServer::start();
    settings(&server, json!({ "ux2": true }));
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    wait_list(&server, "two tabs", |l| tabs(l).len() == 2);
    // UX 2.0 taken up: each tab has its header.
    client.wait_text("M-c Claude");
    open(&mut client);
    // The box, from the view's menu: none of board 11's four is on here,
    // so nothing is picked.
    client.type_text("m");
    pick(&mut client, "Several views at once");
    client.wait_text("Views at once");
    client.wait_text("☐ Tabs");
    // The tabs, then the next view, picked and shown.
    client.type_text(" j \r");
    let screen = client.wait_text("2 views at once");
    assert!(
        screen
            .iter()
            .any(|l| l.contains(" TABS") && l.contains("2 tabs") && l.contains('│')),
        "{}",
        screen.join("\n")
    );
    client.wait_text("tab next view");
    // Kept for next time, after a restart too.
    let kept = server.state.join("mission-grid.json");
    let choice: Value = serde_json::from_str(&std::fs::read_to_string(&kept).unwrap()).unwrap();
    assert_eq!(choice["views"][0], "tabs", "{choice}");
    assert_eq!(choice["views"].as_array().unwrap().len(), 2, "{choice}");
    // Tab gives the focus to the other view and back.
    client.type_text("\t");
    client.type_text("\t");
    // Opened again, it shows the last choice.
    client.type_text("m");
    pick(&mut client, "Several views at once");
    let screen = client.wait_text("Views at once");
    assert!(
        screen
            .iter()
            .any(|l| l.contains("☑ Tabs") && l.contains('1')),
        "{}",
        screen.join("\n")
    );
    client.type_text("\r");
    client.wait_text("2 views at once");
    // Esc goes back to one view, a second closes mission control.
    client.type_text("\x1b");
    client.wait_for("one view again", |s| {
        s.iter().any(|l| l.contains("MISSION CONTROL"))
            && !s.iter().any(|l| l.contains("views at once"))
    });
    client.type_text("\x1b");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
}

/// A server with three tabs, the first in front, and a client on it.
fn three_tabs(extra: Value) -> (TestServer, TestClient) {
    let (server, mut client) = two_tabs(extra);
    server.ok(&["new-tab", "-n", "third"]);
    client.wait_text("3:third");
    client.type_text(&format!("{ALT}1"));
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    (server, client)
}

fn closed(s: &[String]) -> bool {
    !s.iter().any(|l| l.contains("MISSION CONTROL"))
}

#[test]
fn m_number_closes_it_and_goes_to_that_tab_as_keys_json_binds_it() {
    let (server, mut client) = three_tabs(json!({}));
    open(&mut client);
    client.type_text(&format!("{ALT}3"));
    client.wait_for("mission control closed", closed);
    wait_list(&server, "the third tab in front", |l| {
        tabs(l)[2]["current"] == true
    });
    // While a filter is typed too.
    open(&mut client);
    client.type_text("/");
    client.type_text(&format!("{ALT}1"));
    client.wait_for("mission control closed", closed);
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    client.type_text(&format!("{ALT}3"));
    wait_list(&server, "the third tab in front", |l| {
        tabs(l)[2]["current"] == true
    });
    // A number with no tab closes it and stays where it was.
    open(&mut client);
    client.type_text(&format!("{ALT}9"));
    client.wait_for("mission control closed", closed);
    assert_eq!(tabs(&server.list())[2]["current"], true, "nowhere gone");
    // M-3 bound to something else does that instead.
    let config = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("keys.json"), r#"{"root": {"M-3": "new-tab"}}"#).unwrap();
    std::thread::sleep(Duration::from_millis(2500) * common::patience());
    client.type_text(&format!("{ALT}1"));
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    open(&mut client);
    client.type_text(&format!("{ALT}3"));
    let screen = client.screen();
    assert!(!closed(&screen), "still open:\n{}", screen.join("\n"));
    assert_eq!(tabs(&server.list())[0]["current"], true, "no tab selected");
}

#[test]
fn m_p_opens_the_palette_over_it_and_a_pick_runs_there_or_in_the_pane() {
    let (server, mut client) = two_tabs(json!({}));
    open(&mut client);
    // Esc leaves the view as it was.
    client.type_text(&format!("{ALT}p"));
    let screen = client.wait_text("action>");
    assert!(!closed(&screen), "beneath it:\n{}", screen.join("\n"));
    client.type_text("\x1b");
    let screen = client.wait_for("the palette gone", |s| {
        !s.iter().any(|l| l.contains("action>"))
    });
    assert!(!closed(&screen), "still open:\n{}", screen.join("\n"));
    // One of its views shows in it.
    client.type_text(&format!("{ALT}p"));
    client.wait_text("action>");
    client.type_text("mission control: landing");
    client.wait_text("› Mission control: Landing");
    client.type_text("\r");
    client.wait_text("Nothing is landing now.");
    // Anything else runs in the pane it was opened over, once closed.
    client.type_text(&format!("{ALT}p"));
    client.wait_text("action>");
    client.type_text("split pane above");
    client.wait_text("› Split pane above");
    client.type_text("\r");
    client.wait_for("mission control closed", closed);
    wait_list(&server, "the second tab split", |l| {
        tabs(l)[1]["panes"].as_array().unwrap().len() == 2
    });
}

#[test]
fn m_p_lists_views_beyond_the_old_twelve_and_opens_or_switches_to_them() {
    let (_server, mut client) = two_tabs(json!({"reminders": true}));
    // Closed, it opens mission control on the view picked.
    client.type_text(&format!("{ALT}p"));
    client.wait_text("action>");
    client.type_text("mission control: reminders");
    client.wait_text("› Mission control: Reminders");
    client.type_text("\r");
    client.wait_text("No reminders.");
    // Open, it switches to the one picked.
    client.type_text(&format!("{ALT}p"));
    client.wait_text("action>");
    client.type_text("mission control: notifications");
    client.wait_text("› Mission control: Notifications");
    client.type_text("\r");
    let screen = client.wait_text("No notifications");
    assert!(!closed(&screen), "still open:\n{}", screen.join("\n"));
}

#[test]
fn with_ux2_a_click_on_a_tab_in_the_sidebar_closes_it_and_goes_there() {
    let server = TestServer::start();
    settings(&server, json!({"ux2": true, "sidebar": true}));
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    server.ok(&["new-tab", "-n", "third"]);
    let list = wait_list(&server, "three tabs and the sidebar", |l| {
        tabs(l).len() == 3 && l["sessions"][0]["sidebar"]["columns"].as_u64().unwrap_or(0) > 0
    });
    let cols = list["sessions"][0]["sidebar"]["columns"].as_u64().unwrap() as usize;
    // UX 2.0 taken up: the tab bar gone from the bottom.
    client.wait_for("the tab bar gone", |s| {
        !s.iter().any(|l| l.contains("2:second"))
    });
    let screen = open(&mut client);
    let row = screen
        .iter()
        .position(|l| l.chars().take(cols).collect::<String>().contains("second"))
        .unwrap_or_else(|| panic!("the second tab's row:\n{}", screen.join("\n")));
    // The wheel over the sidebar leaves mission control open.
    client.send(format!("\x1b[<65;3;{}M", row + 1).as_bytes());
    std::thread::sleep(Duration::from_millis(300) * common::patience());
    let screen = client.screen();
    assert!(!closed(&screen), "still open:\n{}", screen.join("\n"));
    client.send(format!("\x1b[<0;3;{}M\x1b[<0;3;{}m", row + 1, row + 1).as_bytes());
    client.wait_for("mission control closed", closed);
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
}

/// The bar's second row, the views of the group in front and its keys.
fn views_row(s: &[String]) -> String {
    s.iter()
        .find(|l| l.contains("M-[ ] groups"))
        .cloned()
        .unwrap_or_default()
}

#[test]
fn with_ux2_m_brackets_move_between_groups_and_stay_the_tabs_outside_it() {
    let (server, mut client) = two_tabs_ux2();
    let screen = open(&mut client);
    assert!(views_row(&screen).contains("Tabs"), "{}", screen.join("\n"));
    // M-[ round to the last group, System, and M-] back to Work.
    client.type_text(&format!("{ALT}["));
    client.wait_for("System's views", |s| views_row(s).contains("Settings"));
    client.type_text(&format!("{ALT}]"));
    client.wait_for("Work's views", |s| views_row(s).contains("Tabs"));
    // `}` and `{` leave the view as it is.
    client.type_text("{");
    std::thread::sleep(Duration::from_millis(300) * common::patience());
    let screen = client.screen();
    assert!(views_row(&screen).contains("Tabs"), "{}", screen.join("\n"));
    assert_eq!(
        tabs(&server.list())[1]["current"],
        true,
        "the tab unchanged"
    );
    // Closed, M-[ goes back through the tabs again, to the first.
    client.type_text("q");
    client.wait_for("mission control closed", closed);
    client.type_text(&format!("{ALT}["));
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
}

/// Where a text first shows on the screen, from the top: its row and the
/// column it starts at.
fn find_on(screen: &[String], text: &str) -> Option<(usize, usize)> {
    screen.iter().enumerate().find_map(|(row, line)| {
        let at = line.find(text)?;
        Some((row, line[..at].chars().count()))
    })
}

/// A left click at a 0-based cell.
fn click(client: &mut TestClient, (row, col): (usize, usize)) {
    client.send(
        format!(
            "\x1b[<0;{};{}M\x1b[<0;{};{}m",
            col + 1,
            row + 1,
            col + 1,
            row + 1
        )
        .as_bytes(),
    );
}

/// The top-left corner of the box with this title.
fn corner_of(screen: &[String], title: &str) -> (usize, usize) {
    find_on(screen, &format!("╭─ {title} "))
        .unwrap_or_else(|| panic!("no box {title}:\n{}", screen.join("\n")))
}

/// Whether the box with this title is in the middle of the screen, as a
/// box opened by a key is.
fn in_the_middle(screen: &[String], title: &str) -> bool {
    let (top, left) = corner_of(screen, title);
    let bottom = (top..screen.len())
        .find(|&r| screen[r].chars().nth(left) == Some('╰'))
        .unwrap();
    top == (screen.len() - (bottom + 1 - top)) / 2
}

#[test]
fn a_menu_a_click_opens_opens_beside_the_click_and_enters_in_the_middle() {
    let (_server, mut client) = two_tabs(json!({}));
    let screen = open(&mut client);
    let card = find_on(&screen, "second").unwrap();
    // A click chooses the card, a second opens its menu, beside the click.
    click(&mut client, card);
    click(&mut client, card);
    let screen = client.wait_text("Go to tab 2");
    assert_eq!(corner_of(&screen, "2 second"), (card.0 + 1, card.1 + 1));
    // A question an option's click opens opens beside that option.
    let option = find_on(&screen, "Close the tab").unwrap();
    click(&mut client, option);
    let screen = client.wait_text("Close tab 2 second?");
    // With no room to its right, it opens to the option's left.
    let (top, left) = corner_of(&screen, "Are you sure?");
    let right = screen[top]
        .chars()
        .skip(left)
        .position(|c| c == '╮')
        .unwrap()
        + left;
    assert_eq!(
        (top, right),
        (option.0 + 1, option.1 - 1),
        "{}",
        screen.join("\n")
    );
    client.type_text("n");
    client.wait_for("the question closed", |s| {
        !s.iter().any(|l| l.contains("Close tab 2"))
    });
    // Enter opens it in the middle.
    client.type_text("\r");
    let screen = client.wait_text("Go to tab 2");
    assert!(in_the_middle(&screen, "2 second"), "{}", screen.join("\n"));
}

#[test]
fn with_boxes_near_click_off_a_clicks_menu_opens_in_the_middle() {
    let (_server, mut client) = two_tabs(json!({"boxes_near_click": false}));
    let screen = open(&mut client);
    let card = find_on(&screen, "second").unwrap();
    click(&mut client, card);
    click(&mut client, card);
    let screen = client.wait_text("Go to tab 2");
    assert!(in_the_middle(&screen, "2 second"), "{}", screen.join("\n"));
}

/// A right press at a cell of the screen, counted from 0.
fn right_press(client: &mut TestClient, col: usize, row: usize) {
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
}

/// The column, counted from 0, where `text` starts on a screen's line.
fn column_of(line: &str, text: &str) -> usize {
    let at = line.find(text).unwrap();
    switchboard::ui::draw::width(&line[..at])
}

// A right press on an item opens its menu, on a view's empty part the
// view's, and on the bar nothing; with a menu open, inside it nothing and
// outside it closes the menu without opening another.
#[test]
fn a_right_press_opens_an_items_menu_or_the_views() {
    let notes = common::TempDir::new("sbrc");
    std::fs::write(notes.path().join("Plan.md"), "# Plan\n").unwrap();
    let server = TestServer::start_with_env(&[("NOTES_DIR", notes.path().to_str().unwrap())]);
    std::fs::create_dir_all(&server.state).unwrap();
    std::fs::write(
        server.state.join("new-notes.json"),
        json!([{ "path": "Plan.md", "title": "Plan", "source": "Fathom",
            "at": switchboard::util::now_ms() }])
        .to_string(),
    )
    .unwrap();
    settings(&server, json!({ "new_notes": true }));
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    open(&mut client);
    client.type_text("]");
    let screen = client.wait_text("Plan  shown");
    let row = screen
        .iter()
        .rposition(|l| l.contains("Plan  shown"))
        .unwrap();
    let col = column_of(&screen[row], "Plan  shown") + 4;
    let menu_up = |s: &[String]| s.iter().any(|l| l.contains("Open it in Notes"));
    let view_menu_up = |s: &[String]| {
        s.iter()
            .any(|l| l.contains("Dismiss every notification shown"))
    };

    // On the notification: its menu, its options as Enter's.
    right_press(&mut client, col, row);
    let screen = client.wait_for("the notification's menu", menu_up);
    assert!(!view_menu_up(&screen), "{}", screen.join("\n"));
    // Inside the menu: nothing.
    let inside = screen
        .iter()
        .position(|l| l.contains("Open it in Notes"))
        .unwrap();
    right_press(
        &mut client,
        column_of(&screen[inside], "Open it in Notes"),
        inside,
    );
    std::thread::sleep(Duration::from_millis(300) * common::patience());
    let screen = client.screen();
    assert!(menu_up(&screen), "still open:\n{}", screen.join("\n"));
    // Outside it: closed, and nothing else opened.
    right_press(&mut client, 139, 30);
    client.wait_for("the menu closed", |s| !menu_up(s));
    std::thread::sleep(Duration::from_millis(300) * common::patience());
    let screen = client.screen();
    assert!(!view_menu_up(&screen), "{}", screen.join("\n"));
    assert!(
        !closed(&screen),
        "mission control open:\n{}",
        screen.join("\n")
    );

    // On the view's empty part: the view's menu.
    right_press(&mut client, col, 30);
    let screen = client.wait_for("the view's menu", view_menu_up);
    assert!(!menu_up(&screen), "{}", screen.join("\n"));
    client.type_text("\x1b");
    client.wait_for("the view's menu closed", |s| !view_menu_up(s));

    // On the bar: nothing.
    let screen = client.screen();
    let bar = screen
        .iter()
        .position(|l| l.contains("MISSION CONTROL"))
        .unwrap();
    right_press(&mut client, column_of(&screen[bar], "MISSION CONTROL"), bar);
    std::thread::sleep(Duration::from_millis(300) * common::patience());
    let screen = client.screen();
    assert!(
        !menu_up(&screen) && !view_menu_up(&screen) && !closed(&screen),
        "{}",
        screen.join("\n")
    );
}
