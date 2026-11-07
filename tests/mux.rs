//! The multiplexer as someone uses it: a server of the test's own, a client
//! attached to it, keys typed and the screen read back.

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

fn tabs(list: &serde_json::Value) -> usize {
    list["sessions"][0]["tabs"].as_array().unwrap().len()
}

#[test]
fn attaching_shows_a_shell_and_the_status_line() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    let screen = client.wait_text("$");
    assert!(
        screen[23].contains("main"),
        "the status line names the session: {:?}",
        screen[23]
    );
    client.type_text("echo hello-$((40+2))\r");
    client.wait_text("hello-42");
}

#[test]
fn splitting_tabs_and_closing() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    // M-L: a pane to the right.
    client.type_text(&format!("{ALT}L"));
    client.wait_for("a second pane", |_| panes(&server.list()) == 2);
    let screen = client.wait_for("a line between the panes", |s| {
        s.iter().take(28).any(|l| l.contains('│'))
    });
    assert!(screen.iter().filter(|l| l.contains('$')).count() >= 1);
    // A new tab from a command, then M-[ back to the first.
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("2:second");
    assert_eq!(tabs(&server.list()), 2);
    client.type_text(&format!("{ALT}["));
    client.wait_for("the first tab current", |_| {
        server.list()["sessions"][0]["tabs"][0]["current"] == true
    });
    // M-w closes the focused pane; the tab keeps its other.
    client.type_text(&format!("{ALT}w"));
    client.wait_for("one pane left in the first tab", |_| {
        server.list()["sessions"][0]["tabs"][0]["panes"]
            .as_array()
            .unwrap()
            .len()
            == 1
    });
}

#[test]
fn a_program_ending_closes_its_tab() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    server.ok(&["new-tab", "--", "sh", "-c", "exit 0"]);
    client.wait_for("the short-lived tab gone", |_| tabs(&server.list()) == 1);
}

#[test]
fn send_keys_and_capture_pane() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    let list = server.list();
    let pane = list["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["send-keys", "-t", &pane, "-l", "echo captured-text"]);
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    client.wait_text("captured-text");
    let text = server.ok(&["capture-pane", "-t", &pane]);
    assert!(text.contains("captured-text"), "{text}");
}

#[test]
fn detaching_leaves_it_running_and_attaching_again_shows_it() {
    let mut server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    client.type_text("echo still-here\r");
    client.wait_text("still-here");
    // The prefix, then d.
    client.type_text(&format!("{ALT}b"));
    client.type_text("d");
    let said = client.wait_exit().unwrap_or_default();
    assert!(said.contains("detached"), "{said}");
    assert!(server.running());
    let mut again = server.attach(80, 24);
    again.wait_text("still-here");
}

#[test]
fn options_are_set_on_panes_and_tabs() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    let list = server.list();
    let pane = list["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let tab = list["sessions"][0]["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["set", "-t", &pane, "@agent_state", "working"]);
    server.ok(&["set", "-t", &tab, "@tab_name", "Shaping: things"]);
    assert_eq!(
        server.ok(&["show", "-t", &pane, "@agent_state"]).trim(),
        "working"
    );
    assert_eq!(
        server.ok(&["show", "-t", &tab, "@tab_name"]).trim(),
        "Shaping: things"
    );
    server.ok(&["set", "-u", "-t", &pane, "@agent_state"]);
    assert_eq!(server.ok(&["show", "-t", &pane, "@agent_state"]), "");
}

#[test]
fn killing_the_server_ends_its_clients() {
    let mut server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    server.ok(&["kill-server"]);
    client.wait_exit();
    assert!(
        server.wait_exit(std::time::Duration::from_secs(5)),
        "the server ended"
    );
}

#[test]
fn a_command_with_no_server_says_so() {
    let dir = common::TempDir::new("sbt-none");
    let out = std::process::Command::new(common::BIN)
        .arg("list")
        .env("HOME", dir.path())
        .env("SWITCHBOARD_STATE_DIR", dir.path().join("s"))
        .env_remove("SWITCHBOARD")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no server is running"));
}

/// Where a pane is: its tab, its place among the tab's panes, its size.
fn place(list: &serde_json::Value, pane: &str) -> Option<(String, usize, u64, u64)> {
    list["sessions"][0]["tabs"]
        .as_array()?
        .iter()
        .find_map(|t| {
            let panes = t["panes"].as_array()?;
            let at = panes.iter().position(|p| p["id"] == pane)?;
            Some((
                t["id"].as_str()?.to_string(),
                at,
                panes[at]["cols"].as_u64()?,
                panes[at]["rows"].as_u64()?,
            ))
        })
}

#[test]
fn respawn_starts_another_program_in_the_same_pane() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    // A program that takes a second to end once hung up, so its end comes
    // after the one started in its place.
    let pane = server
        .ok(&[
            "split",
            "-d",
            "--",
            "sh",
            "-c",
            "trap 'sleep 1; exit 0' HUP; echo first-program; while :; do sleep 0.1; done",
        ])
        .trim()
        .to_string();
    client.wait_for("the first program", |_| {
        server
            .ok(&["capture-pane", "-t", &pane])
            .contains("first-program")
    });
    let before = place(&server.list(), &pane).expect("the pane is listed");

    // Still running, it is kept unless -k says to end it, as tmux did.
    let (code, _, err) = server.run(&["respawn-pane", "-t", &pane, "--", "sh", "-c", "exit 0"]);
    assert_eq!(code, 1);
    assert!(err.contains("still running"), "{err}");

    server.ok(&[
        "respawn-pane",
        "-k",
        "-t",
        &pane,
        "--",
        "sh",
        "-c",
        "echo second-program; sleep 30",
    ]);
    client.wait_for("the second program in its place", |_| {
        let text = server.ok(&["capture-pane", "-t", &pane]);
        text.contains("second-program") && !text.contains("first-program")
    });
    // The first program's end has come by now, and the pane is still there,
    // where it was, as big as it was.
    std::thread::sleep(std::time::Duration::from_millis(1800));
    assert_eq!(place(&server.list(), &pane), Some(before.clone()));
    assert_eq!(panes(&server.list()), 2);

    // `switchboard respawn` does the same for the skills, ending what runs.
    server.ok(&[
        "respawn",
        "-t",
        &pane,
        "sh",
        "-c",
        "echo third-program; sleep 30",
    ]);
    client.wait_for("the third program", |_| {
        server
            .ok(&["capture-pane", "-t", &pane])
            .contains("third-program")
    });
    assert_eq!(place(&server.list(), &pane), Some(before));
}
