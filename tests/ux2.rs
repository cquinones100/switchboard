//! UX 2.0 as someone sees it, against a server of the test's own: with its
//! setting on, no status line, and a header over the tab's panes naming it
//! with the keys of what it can show in Claude's place; with it off again,
//! the status line back and the panes the whole height.

mod common;

use common::TestServer;
use serde_json::{Value, json};

/// Settings for the tests: no sidebar, desktop notifications or tips, and
/// these.
fn settings(server: &TestServer, extra: Value) {
    let dir = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&dir).unwrap();
    let mut all = json!({"notifications": false, "tips": false, "sidebar": false});
    for (k, v) in extra.as_object().unwrap() {
        all[k] = v.clone();
    }
    std::fs::write(dir.join("settings.json"), all.to_string()).unwrap();
}

#[test]
fn with_ux2_a_header_names_the_tab_in_place_of_the_status_line() {
    let server = TestServer::start();
    settings(&server, json!({"ux2": true}));
    let mut client = server.attach(90, 24);
    client.wait_text("$");
    let screen = client.wait_for("the header and no status line", |s| {
        s[2].contains("M-c  Claude") && !s[23].contains("main")
    });
    assert!(
        screen[2].contains("M-d  Diff") && screen[2].contains("M-v  PR"),
        "every view's key: {:?}",
        screen[2]
    );
    assert!(
        screen[3].starts_with("───"),
        "a rule under the keys: {:?}",
        screen[3]
    );
    // The pane takes the rest: all but the header's four rows.
    client.wait_for("the pane under the header", |_| {
        server.list()["sessions"][0]["tabs"][0]["panes"][0]["rows"] == 20
    });
    // Off again: the status line back, the pane the whole height but it.
    settings(&server, json!({}));
    client.wait_for("the status line back", |s| {
        s[23].contains("main") && !s[2].contains("M-c  Claude")
    });
    client.wait_for("the pane the whole height", |_| {
        server.list()["sessions"][0]["tabs"][0]["panes"][0]["rows"] == 23
    });
}

#[test]
fn with_ux2_and_message_boxes_off_a_message_is_said_in_a_box() {
    let server = TestServer::start();
    settings(&server, json!({"ux2": true, "message_boxes": false}));
    let mut client = server.attach(90, 24);
    client.wait_for("the header", |s| s[2].contains("M-c  Claude"));
    let pid = server.pid();
    // With no status line to put it on, the message goes in a box, and the
    // server goes on.
    server.ok(&["say", "Nothing needs you."]);
    client.wait_text("Nothing needs you.");
    assert_eq!(server.pid(), pid);
    server.list();
}
