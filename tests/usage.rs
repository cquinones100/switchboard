//! How the workspace is used, on a server of the test's own: a tab that
//! closes is logged for the Closed view with its name and directory, and a
//! plugin named in sidebar.json is run with its options on stdin.

mod common;

use std::time::{Duration, Instant};

use common::TestServer;

fn wait_until(what: &str, check: impl Fn() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_closed_tab_is_logged_with_its_name_and_directory() {
    let server = TestServer::start();
    let dir = server.home.join("work");
    std::fs::create_dir_all(&dir).unwrap();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    let made = server.ok(&["new-tab", "-n", "keep me", "-c", dir.to_str().unwrap()]);
    let tab = made.split_whitespace().next().unwrap_or("").to_string();
    server.ok(&["kill-tab", "-t", &tab]);
    let log = server.state.join("closed.jsonl");
    wait_until("the closing in the log", || log.exists());
    let line: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .last()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(line["kind"], "closed");
    assert_eq!(line["tabName"], "keep me");
    assert_eq!(line["path"], dir.to_str().unwrap());
}

#[test]
fn a_named_plugin_runs_with_its_options() {
    use std::os::unix::fs::PermissionsExt;
    let server = TestServer::start();
    let config = server.home.join(".config/switchboard");
    let plugins = config.join("sidebar-plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let seen = server.home.join("seen");
    let script = plugins.join("demo");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncat > '{}'\necho '[{{\"id\":1,\"title\":\"one\"}}]'\n",
            seen.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        config.join("sidebar.json"),
        r#"{ "plugins": { "demo": { "word": "hello" }, "other": false } }"#,
    )
    .unwrap();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    wait_until("the plugin to run", || {
        std::fs::read_to_string(&seen).is_ok_and(|t| t.contains("hello"))
    });
    assert_eq!(
        std::fs::read_to_string(&seen).unwrap(),
        r#"{"word":"hello"}"#
    );
}
