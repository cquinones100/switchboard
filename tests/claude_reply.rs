//! `claude-reply` against a test server whose tab runs a shell, not Claude:
//! the pane to type into is the tab's active one, there is no reply to
//! give, and a tab that isn't there is said to be missing.

mod common;

use std::process::Command;

use common::{BIN, TestServer};

fn reply(server: &TestServer, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(BIN)
        .arg("claude-reply")
        .args(args)
        .env("HOME", &server.home)
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env_remove("SWITCHBOARD")
        .env_remove("SWITCHBOARD_PANE")
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_tab_with_no_claude_has_no_reply_and_its_active_pane_to_type_into() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    let list = server.list();
    let tab = list["sessions"][0]["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let pane = list["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    assert_eq!(
        reply(&server, &["--pane", "1"]),
        (0, pane.clone(), String::new())
    );
    assert_eq!(reply(&server, &["--pane", &tab]).1, pane, "by its id too");
    let (code, out, err) = reply(&server, &["1"]);
    assert_eq!((code, out.as_str()), (1, ""));
    assert_eq!(err, "This tab isn't running Claude.\n");
    assert_eq!(reply(&server, &["9"]).2, "No tab 9.\n");
    assert_eq!(reply(&server, &[]).0, 2, "a tab must be named");
}
