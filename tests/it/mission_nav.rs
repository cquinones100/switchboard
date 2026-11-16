//! With UX 2.0, mission control's groups and views listed down the side
//! (src/mission/nav.rs, boards 64 and 65): in the sidebar's place while it
//! is open, the view in front lit and a one-line header over it in place of
//! the bar; Tab's focus and `j` move through the views, Enter or a click
//! opens one, `‹ Tabs` brings the tabs back; a view turned off isn't
//! listed; with Settings' "Keep the tabs sidebar in mission control" on,
//! the list is a column of mission control's own beside the tabs.

use crate::common;

use common::{TestClient, TestServer};
use serde_json::{Value, json};

const ALT: &str = "\x1b";

fn settings(extra: Value) -> String {
    let mut all = json!({
        "ux2": true,
        "notifications": false,
        "tips": false,
        "github": false,
        "linear": false,
        "view_calendar": false,
        "view_slack": false,
        "view_relay": false,
        "view_reopen": false,
        "view_resources": false,
        "usage": false,
        "view_keys": true,
        "reminders": false,
        "standup_note": false,
        "new_notes": false,
    });
    for (k, v) in extra.as_object().unwrap() {
        all[k] = v.clone();
    }
    all.to_string()
}

/// A server with a second tab, a client on it, and mission control open.
fn opened(extra: Value) -> (TestServer, TestClient, Vec<String>) {
    let server = TestServer::start_with_settings(&settings(extra));
    let mut client = server.attach(160, 40);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("second");
    client.type_text(&format!("{ALT} "));
    let screen = client.wait_text("MISSION CONTROL");
    (server, client, screen)
}

/// The row and column (both from 0) where `text` first shows.
fn at(screen: &[String], text: &str) -> (usize, usize) {
    screen
        .iter()
        .enumerate()
        .find_map(|(y, l)| l.find(text).map(|i| (y, common_width(&l[..i]))))
        .unwrap_or_else(|| panic!("{text} not shown:\n{}", screen.join("\n")))
}

fn common_width(s: &str) -> usize {
    s.chars().count()
}

fn click(client: &mut TestClient, (y, x): (usize, usize)) {
    client.send(format!("\x1b[<0;{};{}M\x1b[<0;{};{}m", x + 1, y + 1, x + 1, y + 1).as_bytes());
}

/// The sidebar's columns, left of its line.
fn side(screen: &[String], line: usize) -> String {
    screen[line].chars().take_while(|c| *c != '│').collect()
}

#[test]
fn the_sidebar_lists_its_groups_and_views_with_a_header_over_the_view_and_no_bar() {
    let (_server, mut client, screen) = opened(json!({}));
    let all = screen.join("\n");
    assert!(side(&screen, 0).contains("‹ Tabs"), "{all}");
    for label in ["MISSION CONTROL", "WORK", "SYSTEM"] {
        assert!(
            (0..screen.len()).any(|y| side(&screen, y).trim() == label),
            "{label} in the sidebar:\n{all}"
        );
    }
    // The Tabs view in front, lit in the list and named in the header.
    let (y, _) = at(&screen, "▌  Tabs");
    assert!(side(&screen, y).contains("Tabs"), "{all}");
    assert!(
        screen[0].contains("│ Tabs   Work"),
        "the header: {:?}",
        screen[0]
    );
    assert!(!all.contains("M-[ M-]"), "no bar:\n{all}");
    // The tabs' list gone from the sidebar meanwhile.
    assert!(
        !(0..screen.len()).any(|y| side(&screen, y).contains("second")),
        "{all}"
    );
    // Esc brings the tabs back.
    client.type_text("q");
    let screen = client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    assert!(!screen.join("\n").contains("‹ Tabs"));
}

#[test]
fn tab_then_j_and_enter_opens_the_next_view_and_a_click_opens_one() {
    let (_server, mut client, _) = opened(json!({ "tab_focus": true }));
    // Tab's first stop is the list's first view; j moves to the next.
    client.type_text("\t");
    client.type_text("j");
    client.type_text("\r");
    let screen = client.wait_for("the next view open", |s| !s[0].contains("│ Tabs   Work"));
    let header = screen[0].clone();
    assert!(
        header.contains("   Work") || header.contains("   Code"),
        "{header}"
    );
    // A click on Keys' row opens it.
    let keys = at(&screen, " Keys");
    click(&mut client, keys);
    client.wait_for("Keys open", |s| s[0].contains("│ Keys   System"));
}

#[test]
fn a_click_on_back_to_the_tabs_closes_mission_control() {
    let (_server, mut client, screen) = opened(json!({}));
    let back = at(&screen, "‹ Tabs");
    click(&mut client, back);
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
}

#[test]
fn a_view_turned_off_is_not_listed() {
    let (_server, _client, screen) = opened(json!({ "view_keys": false }));
    assert!(
        !(0..screen.len()).any(|y| side(&screen, y).trim_start().starts_with("Keys")),
        "{}",
        screen.join("\n")
    );
}

#[test]
fn keeping_the_tabs_sidebar_lists_the_views_in_a_column_of_its_own_beside_it() {
    let (_server, _client, screen) = opened(json!({ "mission_keep_tabs_sidebar": true }));
    let all = screen.join("\n");
    assert!(all.contains("MISSION CONTROL"), "{all}");
    assert!(!all.contains("‹ Tabs"), "{all}");
    // The tabs sidebar still lists the tabs, left of the column.
    let (y, x) = at(&screen, "MISSION CONTROL");
    assert_eq!(y, 0);
    assert!(x > 30, "the column right of the sidebar:\n{all}");
    assert!(
        (0..screen.len()).any(|y| side(&screen, y).contains("2 second")),
        "the tabs in the sidebar:\n{all}"
    );
}

/// `C-w h` from the view's leftmost pane gives the list of views the keys,
/// whose `j` and Enter open the next view, with the list in the sidebar's
/// place or in a column of its own.
fn c_w_h_reaches_the_list(extra: Value) {
    let (_server, mut client, _) = opened(extra);
    client.type_text("\x17");
    client.type_text("h");
    client.type_text("j");
    client.type_text("\r");
    let screen = client.wait_for("the next view open", |s| !s[0].contains("│ Tabs   Work"));
    let header = screen[0].clone();
    assert!(
        header.contains("   Work") || header.contains("   Code"),
        "{header}"
    );
}

#[test]
fn c_w_h_gives_the_list_of_views_the_keys() {
    c_w_h_reaches_the_list(json!({}));
}

#[test]
fn c_w_h_reaches_the_column_of_views_beside_the_kept_tabs_sidebar() {
    c_w_h_reaches_the_list(json!({ "mission_keep_tabs_sidebar": true }));
}

/// The view the header over mission control names.
fn header_view(screen: &[String]) -> String {
    let after = screen[0].split('│').nth(1).unwrap_or_default().trim_start();
    after
        .split("   ")
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Waits for the header to name a view other than `not`, and gives it.
fn next_header(client: &mut TestClient, not: &str) -> String {
    let screen = client.wait_for("another view in front", |s| header_view(s) != not);
    header_view(&screen)
}

#[test]
fn j_in_the_list_shows_each_view_and_back_skips_the_views_walked_past() {
    let (_server, mut client, screen) = opened(json!({ "tab_focus": true }));
    assert_eq!(header_view(&screen), "Tabs");
    client.type_text("\t");
    let started = std::time::Instant::now();
    client.type_text("j");
    let first = next_header(&mut client, "Tabs");
    // A second j, then back at once: the Map walked past isn't a step,
    // unless the computer was too slow for the second j to come within
    // the second the cursor rests.
    let quick = started.elapsed() < std::time::Duration::from_millis(900);
    client.type_text("j");
    let second = next_header(&mut client, &first);
    client.type_text(&format!("{ALT}{{"));
    let back = next_header(&mut client, &second);
    if quick {
        assert_eq!(back, "Tabs", "one step back from {second}, past {first}");
    } else {
        assert_eq!(back, first);
    }
}

#[test]
fn tab_from_the_list_goes_into_the_view_and_shift_tab_comes_back_to_the_list() {
    let (_server, mut client, _) = opened(json!({ "tab_focus": true }));
    client.type_text("\t");
    client.type_text("j");
    let first = next_header(&mut client, "Tabs");
    // Tab into the view and back leave the list's cursor where it was, so
    // the next j shows the view after it.
    client.type_text("\t");
    client.type_text("\x1b[Z");
    client.type_text("j");
    let second = next_header(&mut client, &first);
    assert_ne!(second, "Tabs");
    // k back along the list shows the view before.
    client.type_text("k");
    assert_eq!(next_header(&mut client, &second), first);
}

#[test]
fn with_the_list_not_following_j_only_moves_the_cursor() {
    let (_server, mut client, _) =
        opened(json!({ "tab_focus": true, "mission_nav_follow": false }));
    client.type_text("\t");
    client.type_text("j");
    std::thread::sleep(std::time::Duration::from_millis(
        300 * u64::from(common::patience()),
    ));
    assert_eq!(header_view(&client.screen()), "Tabs");
    client.type_text("\r");
    next_header(&mut client, "Tabs");
}
