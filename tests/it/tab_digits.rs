//! M-1 to M-9 followed by digits typed within Settings' `tab_digit_delay`
//! go to a tab past 9 (src/mux/tab_digits.rs): with tab 16 open, `M-1` `6`
//! goes there and types nothing, and `M-1` alone goes to tab 1 once the wait
//! passes; with no tab past 9, `M-1` goes at once and the digit after it
//! reaches the pane.

use crate::common;

use common::{TestClient, TestServer};
use serde_json::{Value, json};

/// Every open tab, the first session's.
fn tabs(server: &TestServer) -> Vec<Value> {
    server.list()["sessions"][0]["tabs"]
        .as_array()
        .unwrap()
        .clone()
}

fn current_number(server: &TestServer) -> i64 {
    tabs(server).iter().find(|t| t["current"] == true).unwrap()["index"]
        .as_i64()
        .unwrap()
}

/// The pane of the tab numbered `n`.
fn pane_of(server: &TestServer, n: i64) -> String {
    tabs(server).iter().find(|t| t["index"] == n).unwrap()["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The last line a pane shows with anything on it.
fn last_line(server: &TestServer, pane: &str) -> String {
    let text = server.ok(&["capture-pane", "-t", pane]);
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default()
        .trim_end()
        .to_string()
}

/// A server with the shortest wait, its client at the first tab's prompt.
fn start() -> (TestServer, TestClient) {
    let server = TestServer::start_with_settings(&json!({"tab_digit_delay": "250"}).to_string());
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    (server, client)
}

/// Opens a tab and gives it number `n`, waiting for its prompt.
fn open_numbered(server: &TestServer, client: &mut TestClient, n: i64) {
    let opened = server.ok(&["new-tab", "-d"]);
    let pane = tabs(server)
        .iter()
        .find(|t| t["id"] == opened.split_whitespace().next().unwrap())
        .unwrap()["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["set", "-t", &pane, "@tab_number", &n.to_string()]);
    client.wait_for(&format!("tab {n} numbered"), |_| {
        tabs(server).iter().any(|t| t["index"] == n)
    });
    client.wait_for(&format!("tab {n}'s prompt"), |_| {
        last_line(server, &pane).ends_with('$')
    });
}

#[test]
fn m_1_then_a_digit_goes_to_a_tab_past_9_and_alone_to_tab_1_after_the_wait() {
    let (server, mut client) = start();
    open_numbered(&server, &mut client, 10);
    open_numbered(&server, &mut client, 16);

    client.send(b"\x1b16");
    client.wait_for("tab 16 in front", |_| current_number(&server) == 16);
    let pane = pane_of(&server, 16);
    assert!(
        last_line(&server, &pane).ends_with('$'),
        "the 6 was taken for the number: {}",
        last_line(&server, &pane)
    );

    client.send(b"\x1b10");
    client.wait_for("tab 10 in front", |_| current_number(&server) == 10);

    client.send(b"\x1b1");
    client.wait_for("tab 1 in front once the wait passes", |_| {
        current_number(&server) == 1
    });
    let pane = pane_of(&server, 1);
    assert!(last_line(&server, &pane).ends_with('$'));
}

#[test]
fn with_no_tab_past_9_m_1_goes_at_once_and_the_digit_reaches_the_pane() {
    let (server, mut client) = start();
    open_numbered(&server, &mut client, 2);
    server.ok(&["press", "select-tab-2"]);
    client.wait_for("tab 2 in front", |_| current_number(&server) == 2);

    client.send(b"\x1b16");
    client.wait_for("tab 1 in front", |_| current_number(&server) == 1);
    let pane = pane_of(&server, 1);
    client.wait_for("the 6 typed in tab 1", |_| {
        last_line(&server, &pane).ends_with("$ 6")
    });
}

#[test]
fn go_to_a_tab_by_number_from_the_palette_still_asks_in_a_box() {
    let (server, mut client) = start();
    server.ok(&["press", "go-to-tab"]);
    client.wait_text("Its number");
}
