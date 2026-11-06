//! The boxes over the workspace as someone uses them: the palette, a
//! message, a tip and the question before closing a busy pane, each through
//! a server and a client of the test's own, keys typed and the screen read
//! back.

mod common;

use common::TestServer;

const ALT: &str = "\x1b";

fn panes(list: &serde_json::Value) -> usize {
    list["sessions"][0]["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["panes"].as_array().unwrap().len())
        .sum()
}

/// The command in front in the session's focused pane.
fn front_command(list: &serde_json::Value) -> String {
    let tabs = list["sessions"][0]["tabs"].as_array().unwrap();
    let tab = tabs.iter().find(|t| t["current"] == true).unwrap();
    let pane = tab["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["active"] == true)
        .unwrap();
    pane["command"].as_str().unwrap_or("").to_string()
}

#[test]
fn the_palette_filters_as_you_type_and_runs_the_pick() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    client.type_text(&format!("{ALT}p"));
    let screen = client.wait_text("action>");
    assert!(
        screen.iter().any(|l| l.contains("Actions")),
        "the box has its title:\n{}",
        screen.join("\n")
    );
    client.type_text("split");
    client.wait_for("only the splits listed", |s| {
        s.iter().any(|l| l.contains("Split pane above"))
            && !s.iter().any(|l| l.contains("Close the pane"))
    });
    let screen = client.screen();
    assert!(
        screen.iter().any(|l| l.contains("› Split pane above")),
        "the first is chosen:\n{}",
        screen.join("\n")
    );
    client.type_text("\r");
    client.wait_for("a second pane", |_| panes(&server.list()) == 2);
    client.wait_for("the palette gone", |s| {
        !s.iter().any(|l| l.contains("action>"))
    });
    // The pick is remembered, and what was typed before it.
    let recent =
        std::fs::read_to_string(server.state.join("palette/recent")).expect("the recent file");
    assert_eq!(recent.lines().next(), Some("split-up"));
    let queries = std::fs::read_to_string(server.state.join("palette/queries")).unwrap();
    assert_eq!(queries, "split\tsplit-up\n");
}

#[test]
fn a_message_shows_until_a_key_which_goes_no_further() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    server.ok(&["say", "Nothing needs you."]);
    client.wait_text("Nothing needs you.");
    client.type_text("x");
    client.wait_for("the message gone", |s| {
        !s.iter().any(|l| l.contains("Nothing needs you."))
    });
    client.type_text("echo said-$((1+1))\r");
    client.wait_text("said-2");
    let pane = server.ok(&["capture-pane"]);
    assert!(
        !pane.contains("xecho"),
        "the key that closed the box didn't reach the shell:\n{pane}"
    );
}

#[test]
fn a_tip_shows_once_and_passes_its_key_on() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    server.ok(&["tip", "pinned"]);
    let screen = client.wait_text("Pinned.");
    assert!(
        screen[0].ends_with("╮") && screen[0].contains("tip"),
        "in the top-right corner: {:?}",
        screen[0]
    );
    client.type_text("echo tipped-$((2+2))\r");
    client.wait_text("tipped-4");
    client.wait_for("the tip gone", |s| !s.iter().any(|l| l.contains("Pinned.")));
    let seen = std::fs::read_to_string(server.state.join("tips")).unwrap();
    assert_eq!(seen, "pinned\n");
    // Shown once: asked again, it stays away.
    server.ok(&["tip", "pinned"]);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!client.screen().iter().any(|l| l.contains("Pinned.")));
    // Until the tips are reset.
    server.ok(&["tip", "reset"]);
    server.ok(&["tip", "pinned"]);
    client.wait_text("Pinned.");
}

#[test]
fn closing_a_busy_pane_asks_first() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    // A shell at its prompt closes at once.
    client.type_text(&format!("{ALT}L"));
    client.wait_for("a second pane", |_| panes(&server.list()) == 2);
    client.type_text(&format!("{ALT}w"));
    client.wait_for("the shell's pane closed", |_| panes(&server.list()) == 1);
    // One running a program asks.
    client.type_text(&format!("{ALT}L"));
    client.wait_for("a second pane", |_| panes(&server.list()) == 2);
    client.type_text("sleep 100\r");
    client.wait_for("sleep in front", |_| {
        front_command(&server.list()) == "sleep"
    });
    client.type_text(&format!("{ALT}w"));
    let screen = client.wait_text("Close pane? [Y/n]");
    assert!(
        screen.iter().any(|l| l.contains("sleep 100")),
        "it says what runs:\n{}",
        screen.join("\n")
    );
    client.type_text("n");
    client.wait_for("the question gone", |s| {
        !s.iter().any(|l| l.contains("Close pane?"))
    });
    assert_eq!(panes(&server.list()), 2, "no is no");
    client.type_text(&format!("{ALT}w"));
    client.wait_text("Close pane? [Y/n]");
    client.type_text("\r");
    client.wait_for("the busy pane closed", |_| panes(&server.list()) == 1);
}

#[test]
fn the_list_of_keys_opens_over_the_tab() {
    let server = TestServer::start();
    let mut client = server.attach(100, 40);
    client.wait_text("$");
    client.type_text(&format!("{ALT}b"));
    client.type_text("?");
    let screen = client.wait_text("Split pane to the left");
    assert!(screen.iter().any(|l| l.contains("Keys")));
    client.type_text("q");
    client.wait_for("the list gone", |s| {
        !s.iter().any(|l| l.contains("Split pane to the left"))
    });
}
