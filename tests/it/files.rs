//! Your files from a git repository (src/files), against a server of the
//! test's own whose remote is a bare repository in the test's directory:
//! the clone and the files it takes in, a setting read from and written to
//! it, the change pushed, the remote pulled, one that diverged left alone,
//! leak-check's question before a push, and the setting off.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::common::{self, TestServer};

/// An address leak-check finds, written in two pieces so it finds none in
/// this file.
const ADDRESS: &str = concat!("me@", "corp.test");

/// git in `dir`, with the test's home as its own, so the commits are made
/// under the test's identity and none of yours is read.
fn git(home: &Path, dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A bare repository beside the test's home, and an identity to commit
/// under in the home.
fn remote(server: &TestServer) -> PathBuf {
    std::fs::write(
        server.home.join(".gitconfig"),
        "[user]\n\tname = Someone\n\temail = someone@example.com\n[init]\n\tdefaultBranch = main\n",
    )
    .unwrap();
    let remote = server.dir.path().join("remote.git");
    std::fs::create_dir_all(&remote).unwrap();
    git(
        &server.home,
        &remote,
        &["init", "--quiet", "--bare", "-b", "main"],
    );
    remote
}

fn name_the_repository(server: &TestServer, remote: &Path) {
    switchboard::util::write_atomic(
        server.home.join(".config/switchboard/files.json"),
        serde_json::json!({ "repo": remote }).to_string(),
    )
    .unwrap();
}

/// Waits until `done`, saying what for when it never is.
fn wait(server: &TestServer, what: &str, done: impl Fn() -> bool) {
    let deadline = common::deadline(Duration::from_secs(20));
    while !done() {
        assert!(
            Instant::now() < deadline,
            "never {what}; server log: {}",
            server.log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn your_files_are_read_from_the_clone_pushed_as_they_change_and_pulled() {
    let server = TestServer::start_with_settings(r#"{"files_from_repo": true, "tips": false}"#);
    let mut client = server.attach(140, 40);
    let home = server.home.clone();
    let remote = remote(&server);
    let local = home.join(".config/switchboard");
    std::fs::write(local.join("browser.json"), "{\"default\": \"Work\"}\n").unwrap();
    std::fs::write(local.join("trusted-dirs"), "~/code/mine\n").unwrap();
    std::fs::write(local.join("secrets.json"), "{}\n").unwrap();
    std::fs::write(local.join("repos.json"), "{}\n").unwrap();
    std::fs::create_dir_all(local.join("sidebar-plugins")).unwrap();
    std::fs::write(local.join("sidebar-plugins/mine"), "#!/bin/sh\n").unwrap();
    name_the_repository(&server, &remote);
    let pushed = |file: &str| {
        git(
            &home,
            &remote,
            &["show", &format!("main:switchboard/{file}")],
        )
    };
    wait(&server, "pushed the files it took in", || {
        !pushed("settings.json").is_empty()
    });
    assert_eq!(
        git(&home, &remote, &["ls-tree", "-r", "--name-only", "main"]),
        "switchboard/browser.json\nswitchboard/settings.json",
        "files.json, trusted-dirs, repos.json, the plugins and secrets.json stay on the computer"
    );
    let clone = home.join(".local/share/switchboard/files");
    assert!(!clone.join("switchboard/trusted-dirs").exists());

    // A setting is read from the clone's copy, and written to it.
    let (_, tips, _) = server.run_binary(&["setting", "tips"]);
    assert_eq!(tips.trim(), "off");
    let (code, _, err) = server.run_binary(&["setting", "tips", "on"]);
    assert_eq!(code, 0, "{err}");
    wait(&server, "pushed the changed setting", || {
        !pushed("settings.json").contains("tips")
    });
    assert_eq!(
        git(&home, &remote, &["log", "-1", "--format=%s", "main"]),
        "Switchboard: settings.json"
    );
    let own = std::fs::read_to_string(local.join("settings.json")).unwrap();
    assert!(
        own.contains("tips"),
        "the computer's own copy is left: {own}"
    );

    // Another computer pushes: the next pull takes it in.
    let other = server.dir.path().join("other");
    git(
        &home,
        server.dir.path(),
        &[
            "clone",
            "--quiet",
            remote.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    let theirs = |text: &str, subject: &str| {
        std::fs::write(other.join("switchboard/browser.json"), text).unwrap();
        git(&home, &other, &["commit", "--quiet", "-am", subject]);
        git(&home, &other, &["push", "--quiet", "origin", "main"]);
    };
    theirs("{\"default\": \"Home\"}\n", "theirs");
    server.ok(&["press", "sync-your-files"]);
    let browser = clone.join("switchboard/browser.json");
    wait(&server, "pulled the remote's change", || {
        std::fs::read_to_string(&browser).is_ok_and(|text| text.contains("Home"))
    });
    client.wait_text("up to date");

    // Both change the same file: nothing is merged or pushed, and it says so.
    theirs("{\"default\": \"Theirs\"}\n", "theirs again");
    let tip = git(&home, &remote, &["rev-parse", "main"]);
    std::fs::write(&browser, "{\"default\": \"Mine\"}\n").unwrap();
    client.wait_text("--rebase");
    assert_eq!(git(&home, &remote, &["rev-parse", "main"]), tip);
    assert!(std::fs::read_to_string(&browser).unwrap().contains("Mine"));
}

#[test]
fn what_leak_check_finds_in_your_files_is_asked_about_before_the_push() {
    let server = TestServer::start_with_settings(r#"{"files_from_repo": true, "tips": false}"#);
    let mut client = server.attach(140, 40);
    let home = server.home.clone();
    let remote = remote(&server);
    std::fs::write(
        home.join(".config/switchboard/browser.json"),
        format!("{{\"owner\": \"{ADDRESS}\"}}\n"),
    )
    .unwrap();
    name_the_repository(&server, &remote);
    let screen = client.wait_text("Push your Switchboard files?");
    assert!(
        screen.iter().any(|line| line.contains(ADDRESS)),
        "{screen:#?}"
    );
    assert!(git(&home, &remote, &["branch", "--list"]).is_empty());
    client.send(b"\r");
    wait(&server, "pushed after the yes", || {
        git(&home, &remote, &["ls-tree", "-r", "--name-only", "main"])
            .contains("switchboard/browser.json")
    });
}

#[test]
fn with_the_setting_off_files_json_is_ignored() {
    let server = TestServer::start_with_settings(r#"{"tips": false}"#);
    let mut client = server.attach(140, 40);
    let remote = remote(&server);
    name_the_repository(&server, &remote);
    server.ok(&["press", "sync-your-files"]);
    client.wait_text("(files.json)");
    assert!(!server.home.join(".local/share/switchboard/files").exists());
    assert!(git(&server.home, &remote, &["branch", "--list"]).is_empty());
    let (_, tips, _) = server.run_binary(&["setting", "tips"]);
    assert_eq!(tips.trim(), "off");
}
