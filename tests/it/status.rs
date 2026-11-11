//! The status line as someone sees it, against a server of the test's own:
//! the second status line under the tabs with its setting on, and gone
//! again with it off; the ⌘ Commands button opening the palette when
//! clicked, a click on a tab still going to it, and with UX 2.0 at the end
//! of the tab header's keys, a click there opening and closing the
//! palette; the health light naming
//! what stopped; and the palette's theme toggle setting the theme.

use crate::common;

use common::{TestClient, TestServer};
use serde_json::{Value, json};

const ALT: &str = "\x1b";

/// Settings for the tests: no sidebar, desktop notifications or tips, and
/// these.
fn settings(server: &TestServer, extra: Value) {
    let mut all = json!({"notifications": false, "tips": false, "sidebar": false});
    for (k, v) in extra.as_object().unwrap() {
        all[k] = v.clone();
    }
    common::write_settings(&server.home, all);
}

/// A left click at a cell (0-based), pressed and let go, as SGR mouse.
fn click(client: &mut TestClient, col: usize, row: usize) {
    client.send(format!("\x1b[<0;{};{}M", col + 1, row + 1).as_bytes());
    client.send(format!("\x1b[<0;{};{}m", col + 1, row + 1).as_bytes());
}

/// The column `text` starts at in a line, counted in characters.
fn column(line: &str, text: &str) -> usize {
    let at = line
        .find(text)
        .unwrap_or_else(|| panic!("{text:?} in {line:?}"));
    line[..at].chars().count()
}

#[test]
fn the_second_status_line_shows_under_the_tabs_with_its_setting_on() {
    let server = TestServer::start();
    settings(&server, json!({"second_status": true}));
    let mut client = server.attach(90, 24);
    client.wait_text("$");
    // The tab's label (its directory) and the program in front, under the
    // tabs' row, which moves up one.
    let screen = client.wait_for("the second status line", |s| {
        s[22].contains("main") && s[23].contains("home") && s[23].contains("sh")
    });
    assert!(
        screen[23].starts_with("  home"),
        "the label leads it: {:?}",
        screen[23]
    );
    // The pane is laid out above both.
    client.wait_for("the pane one row shorter", |_| {
        server.list()["sessions"][0]["tabs"][0]["panes"][0]["rows"] == 22
    });
    // Off again: the tabs back on the last row.
    settings(&server, json!({}));
    client.wait_for("one status line", |s| {
        s[23].contains("main") && !s[22].contains("main")
    });
}

#[test]
fn the_commands_button_opens_the_palette_and_a_tab_is_still_gone_to() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(100, 24);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    let screen = client.wait_for("the button and both tabs", |s| {
        s[23].contains("⌘ Commands") && s[23].contains("2:second")
    });
    let line = screen[23].clone();
    // A click on the first tab goes to it.
    click(&mut client, column(&line, "1:") + 1, 23);
    client.wait_for("the first tab current", |_| {
        server.list()["sessions"][0]["tabs"][0]["current"] == true
    });
    // A click on the button opens the palette.
    click(&mut client, column(&line, "Commands"), 23);
    client.wait_text("action>");
    client.type_text(ALT);
    client.wait_for("the palette closed", |s| {
        !s.iter().any(|l| l.contains("action>"))
    });
    // With the button off, nothing is there to click.
    settings(&server, json!({"palette_button": false}));
    client.wait_for("no button", |s| !s[23].contains("Commands"));
}

#[test]
fn with_ux2_the_headers_commands_button_opens_the_palette_and_closes_it() {
    let server = TestServer::start();
    settings(&server, json!({"ux2": true}));
    let mut client = server.attach(120, 30);
    let screen = client.wait_for("the button after the keys", |s| {
        s[2].contains("M-e") && s[2].contains("Commands")
    });
    let line = screen[2].clone();
    assert!(
        column(&line, "Commands") > column(&line, "M-e"),
        "at the end of the keys: {line:?}"
    );
    click(&mut client, column(&line, "Commands"), 2);
    client.wait_text("Actions");
    click(&mut client, column(&line, "Commands"), 2);
    client.wait_for("the palette closed", |s| {
        !s.iter().any(|l| l.contains("Actions"))
    });
    settings(&server, json!({"ux2": true, "palette_button": false}));
    client.wait_for("no button", |s| {
        s[2].contains("M-e") && !s[2].contains("Commands")
    });
}

#[test]
fn the_health_light_names_what_stopped() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(120, 24);
    client.wait_text("$");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    std::fs::create_dir_all(&server.state).unwrap();
    std::fs::write(
        server.state.join("connectors-down"),
        format!("Slack\t{now}\n"),
    )
    .unwrap();
    let relay = server.home.join(".local/state/slack-relay");
    std::fs::create_dir_all(&relay).unwrap();
    std::fs::write(relay.join("events.jsonl"), "").unwrap();
    client.wait_for("the light", |s| {
        s[23].contains("● relay down, Slack not connected")
    });
    // Put right, it goes out.
    std::fs::remove_file(relay.join("events.jsonl")).unwrap();
    std::fs::write(server.state.join("connectors-down"), "").unwrap();
    client.wait_for("the light out", |s| !s[23].contains('●'));
}

#[test]
fn the_palette_toggles_the_theme() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    client.type_text(&format!("{ALT}p"));
    client.wait_text("action>");
    client.type_text("Toggle light");
    client.wait_for("the toggle listed", |s| {
        s.iter()
            .any(|l| l.contains("› Toggle light and dark theme"))
    });
    client.type_text("\r");
    let file = server.home.join(".config/switchboard/settings.json");
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    loop {
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        if saved["theme"] == "light" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the theme never turned light: {saved}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
