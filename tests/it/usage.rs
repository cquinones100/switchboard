//! How the workspace is used, on a server of the test's own: a tab that
//! closes is logged for the Closed view with its name and directory, a
//! plugin named in sidebar.json is run with its options on stdin, and one
//! that asks for it lists its first items in the sidebar.

use crate::common;

use std::time::{Duration, Instant};

use common::TestServer;

fn wait_until(what: &str, check: impl Fn() -> bool) {
    let deadline = common::deadline(Duration::from_secs(15));
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_setting_is_changed_from_the_command_line() {
    let dir = common::TempDir::new("sb-setting");
    let file = dir.path().join("settings.json");
    let run = |args: &[&str]| {
        let out = std::process::Command::new(common::BIN)
            .arg("setting")
            .args(args)
            .env("HOME", dir.path())
            .env("SWITCHBOARD_SETTINGS", &file)
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )
    };
    std::fs::write(&file, "{\"tips\": true}\n").unwrap();
    assert_eq!(run(&["review_security", "off"]), (0, "off".into()));
    assert_eq!(run(&["review_security"]), (0, "off".into()));
    let mine: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(mine["review_security"], false);
    assert!(dir.path().join("settings.json.bak").exists());
    // What isn't a value is refused, and the file left as it was.
    assert_eq!(run(&["review_security", "maybe"]).0, 2);
    assert_eq!(run(&["no_such_setting", "on"]).0, 2);
    assert_eq!(run(&["review_security", "--reset"]), (0, "on".into()));
    let mine: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert!(mine.get("review_security").is_none(), "{mine}");
    // A file that can't be read as JSON is never written over.
    std::fs::write(&file, "not json").unwrap();
    assert_eq!(run(&["review_security", "off"]).0, 1);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "not json");
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
    let server = TestServer::start();
    let config = server.home.join(".config/switchboard");
    let plugins = config.join("sidebar-plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let seen = server.home.join("seen");
    let script = plugins.join("demo");
    common::stand_in::install(
        &script,
        &format!(
            "#!/bin/sh\ncat > '{}'\necho '[{{\"id\":1,\"title\":\"one\"}}]'\n",
            seen.display()
        ),
    );
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

#[test]
fn a_plugin_that_asks_for_it_lists_its_first_items_in_the_sidebar() {
    let server = TestServer::start();
    let config = server.home.join(".config/switchboard");
    let plugins = config.join("sidebar-plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let script = plugins.join("deploys");
    let items = r#"{"title":"Deploys","sidebar":2,"items":[{"id":1,"title":"api rolling out","state":"attention"},{"id":2,"title":"web done","state":"idle"},{"id":3,"title":"jobs queued"}]}"#;
    common::stand_in::install(&script, &format!("#!/bin/sh\necho '{items}'\n"));
    std::fs::write(
        config.join("sidebar.json"),
        r#"{ "plugins": { "deploys": true } }"#,
    )
    .unwrap();
    let mut client = server.attach(140, 36);
    let screen = client.wait_text("DEPLOYS 3 ! 1");
    assert!(screen.iter().any(|l| l.contains("● api rolling out")));
    assert!(screen.iter().any(|l| l.contains("○ web done")));
    assert!(
        !screen.iter().any(|l| l.contains("jobs queued")),
        "only the first two"
    );
}
