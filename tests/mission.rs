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
fn enter_on_a_tabs_card_and_go_to_tab_goes_there() {
    let (server, mut client) = two_tabs(json!({}));
    open(&mut client);
    // The cursor starts on the first card: tab 1's, whose menu Enter opens.
    client.type_text("\r");
    let screen = client.wait_text("Go to tab 1");
    assert!(screen.iter().any(|l| l.contains("Rename it")));
    assert!(screen.iter().any(|l| l.contains("Close the tab")));
    client.type_text("\r");
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    // l to the second card, then its menu's key for going there.
    open(&mut client);
    client.type_text("l");
    client.type_text("\r");
    client.wait_text("Go to tab 2");
    client.type_text("g");
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
    // The first row is its category's heading; j to GitHub, Enter turns it on.
    client.type_text("j");
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
    let screen = client.wait_text("Push notifications");
    assert!(!screen.iter().any(|l| l.contains("INTEGRATIONS")));
    client.type_text("\x1b");
    client.wait_text("INTEGRATIONS");
    client.type_text("\x1b");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
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
    // Typing filters the rows; Enter goes to the chosen one at once, as a
    // picker does.
    client.type_text("second");
    client.wait_for("one row", |s| !s.iter().any(|l| l.contains("tab 1 ")));
    client.type_text("\r");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
}

#[test]
fn a_reminder_set_from_a_tabs_menu_puts_it_under_reminders() {
    let (server, mut client) = two_tabs(json!({}));
    open(&mut client);
    client.type_text("\r");
    client.wait_text("Remind me about it later");
    client.type_text("e");
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
