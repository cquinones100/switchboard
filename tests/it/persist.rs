//! The layout saved and brought back: a server of the test's own is given
//! tabs, splits, names and options, saved, ended, and started again in the
//! same home, as after a restart of the machine.

use crate::common;

use std::time::{Duration, Instant};

use common::TestServer;
use common::stand_in::Claude;
use serde_json::Value;

fn tabs(list: &Value) -> Vec<Value> {
    list["sessions"][0]["tabs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn pane_ids(list: &Value) -> Vec<Vec<String>> {
    tabs(list)
        .iter()
        .map(|t| {
            t["panes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["id"].as_str().unwrap().to_string())
                .collect()
        })
        .collect()
}

#[test]
fn tabs_panes_names_options_and_screens_come_back() {
    let mut server = TestServer::start();
    let proj = server.home.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    // A stand-in for Claude, so the test never starts the real one.
    Claude::new()
        .answer("echo \"resumed $*\"")
        .install(&server.home.join("bin"));

    let mut client = server.attach(100, 30);
    client.wait_text("$");
    client.type_text("echo marker-$((6*7))\r");
    client.wait_text("marker-42");

    let out = server.ok(&["new-tab", "-c", proj.to_str().unwrap(), "-n", "work"]);
    let mut words = out.split_whitespace();
    let (tab, pane) = (words.next().unwrap(), words.next().unwrap());
    let split = server.ok(&["split", "-t", pane, "-D", "-d"]);
    let split = split.trim();
    server.ok(&["set", "-t", tab, "@tab_note", "Today's plan"]);
    // When it opened, set as it opened, and an hour before for the test.
    assert!(
        !server
            .ok(&["show", "-t", tab, "@tab_opened"])
            .trim()
            .is_empty()
    );
    server.ok(&["set", "-t", tab, "@tab_opened", "1700000000"]);
    // Autopilot's marks, as tmux/window-options.sh carried them.
    server.ok(&["set", "-t", tab, "@autopilot_land", "1"]);
    server.ok(&["set", "-t", tab, "@autopilot_autoland", "abc1234"]);
    server.ok(&["set", "-t", split, "@role", "logs"]);
    // A pane running Claude: a process with a session file by its pid.
    server.ok(&[
        "new-tab",
        "-d",
        "-c",
        proj.to_str().unwrap(),
        "--",
        "sh",
        "-c",
        r#"mkdir -p "$HOME/.claude/sessions" && printf '{"sessionId":"sess-abc"}' > "$HOME/.claude/sessions/$$.json" && exec sleep 600"#,
    ]);
    let sessions = server.home.join(".claude/sessions");
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    while std::fs::read_dir(&sessions).map_or(true, |mut d| d.next().is_none()) {
        assert!(
            Instant::now() < deadline,
            "the stand-in never wrote its session file"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    let before = server.list();
    let file = server.ok(&["save"]);
    let saved = std::fs::read_to_string(file.trim()).expect("save prints the file it wrote");
    assert!(
        saved.contains("sess-abc"),
        "the Claude session is saved: {saved}"
    );
    assert!(saved.contains("marker-42"), "what panes show is saved");

    drop(client);
    server.restart();
    let after = server.list();
    let restored = tabs(&after);
    assert_eq!(restored.len(), 3, "every tab is back: {after}");
    assert_eq!(pane_ids(&after), pane_ids(&before), "with the same panes");
    assert_eq!(restored[1]["name"], "work");
    assert_eq!(
        restored[1]["current"], true,
        "the current tab is current again"
    );
    assert_eq!(restored[1]["options"]["@tab_note"], "Today's plan");
    assert_eq!(
        restored[1]["options"]["@tab_opened"], "1700000000",
        "its age goes on from when it opened"
    );
    assert_eq!(restored[1]["options"]["@autopilot_land"], "1");
    assert_eq!(restored[1]["options"]["@autopilot_autoland"], "abc1234");
    assert_eq!(restored[1]["panes"][1]["options"]["@role"], "logs");
    for p in restored[1]["panes"].as_array().unwrap() {
        assert_eq!(
            p["cwd"].as_str(),
            proj.to_str(),
            "each pane in its directory"
        );
    }

    // New ids come after the restored ones.
    let fresh = server.ok(&["new-tab", "-d"]);
    assert!(
        fresh.starts_with("@4 %5"),
        "a new tab after a restore: {fresh}"
    );

    let mut client = server.attach(100, 30);
    server.ok(&["select-tab", "-t", "@1"]);
    client.wait_text("marker-42");
    server.ok(&["select-tab", "-t", "@3"]);
    client.wait_text("resumed --resume sess-abc");
}

#[test]
fn a_save_that_doesnt_parse_is_set_aside_and_the_server_starts_fresh() {
    let mut server = TestServer::start();
    std::fs::write(server.state.join("layout.json"), "{ not a layout").unwrap();
    server.restart();
    assert_eq!(tabs(&server.list()).len(), 0, "nothing is restored");
    assert_eq!(
        std::fs::read_to_string(server.state.join("layout.json.bad")).unwrap(),
        "{ not a layout"
    );
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    assert_eq!(tabs(&server.list()).len(), 1);
}

#[test]
fn nothing_is_restored_with_the_setting_off() {
    let mut server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    server.ok(&["save"]);
    common::write_settings(&server.home, serde_json::json!({ "auto_restore": false }));
    drop(client);
    server.restart();
    assert_eq!(tabs(&server.list()).len(), 0);
}

#[test]
fn a_change_is_saved_without_asking_once_it_holds() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "kept"]);
    let file = server.state.join("layout.json");
    let deadline = common::deadline(std::time::Duration::from_secs(20));
    while !std::fs::read_to_string(&file).is_ok_and(|t| t.contains("\"kept\"")) {
        assert!(Instant::now() < deadline, "the layout was never saved");
        std::thread::sleep(Duration::from_millis(100));
    }
}
