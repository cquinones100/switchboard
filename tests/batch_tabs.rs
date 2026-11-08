//! `wt-batch` and `shape-batch`, run in a pane of a test server of their own
//! (its SWITCHBOARD and SWITCHBOARD_PANE set), in a temporary repository:
//! a tab for each ticket or idea, at the end of the tabs in the order given,
//! focus left where it was, and, for shape-batch, `shape` run in each tab,
//! naming it and starting Claude (a stand-in that writes down its
//! arguments) in the repository.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use common::{BIN, TestServer};
use serde_json::Value;

/// The server's first pane, once a client has made its session.
fn first_pane(server: &TestServer) -> String {
    server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn repo(server: &TestServer) -> PathBuf {
    let repo = server.dir.path().join("repo");
    let ok = Command::new("git")
        .args(["init", "-q"])
        .arg(&repo)
        .stdout(Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(ok);
    repo
}

/// Runs a batch command as the pane `pane` would, in `cwd`.
fn batch(server: &TestServer, pane: Option<&str>, cwd: &Path, args: &[&str]) -> Output {
    let path = std::env::var("PATH").unwrap_or_default();
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .current_dir(cwd)
        .env("HOME", &server.home)
        .env(
            "PATH",
            format!("{}:{path}", server.home.join("bin").display()),
        )
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env("SHELL", "/bin/bash")
        .env_remove("SWITCHBOARD")
        .env_remove("SWITCHBOARD_PANE")
        .stdin(Stdio::null());
    if let Some(pane) = pane {
        cmd.env("SWITCHBOARD", server.socket())
            .env("SWITCHBOARD_PANE", pane);
    }
    cmd.output().unwrap()
}

fn tabs(server: &TestServer) -> Vec<Value> {
    server.list()["sessions"][0]["tabs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[test]
fn wt_batch_opens_a_tab_per_ticket_at_the_end_each_once_and_none_for_a_typo() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_for("a prompt", |s| s.iter().any(|l| l.contains('$')));
    let pane = first_pane(&server);
    let repo = repo(&server);

    let typo = batch(
        &server,
        Some(&pane),
        &repo,
        &["wt-batch", "ABC-1", "not a ticket"],
    );
    assert_eq!(typo.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&typo.stderr).contains("could not parse"));
    assert_eq!(tabs(&server).len(), 1, "no tab opened for any of them");

    let outside = batch(&server, None, &repo, &["wt-batch", "ABC-1"]);
    assert_eq!(outside.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&outside.stderr).contains("not inside Switchboard"));

    let out = batch(
        &server,
        Some(&pane),
        &repo,
        &[
            "wt-batch",
            "abc-1,",
            "https://linear.app/t/issue/ABC-2/title ABC-1",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    let all = tabs(&server);
    assert_eq!(all.len(), 3, "one tab each, ABC-1 once");
    assert!(
        all[0]["current"].as_bool().unwrap(),
        "focus stays where it was"
    );
    for (tab, ticket) in all[1..].iter().zip(["ABC-1", "ABC-2"]) {
        let id = tab["id"].as_str().unwrap();
        assert!(
            said.contains(&format!("Opened {ticket} in window {id}")),
            "{said}"
        );
    }
}

#[test]
fn shape_batch_runs_shape_in_a_tab_per_item_which_names_it_and_starts_claude_there() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_for("a prompt", |s| s.iter().any(|l| l.contains('$')));
    let pane = first_pane(&server);
    let repo = repo(&server);
    let bin = server.home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let args_file = server.home.join("claude-args");
    std::fs::write(
        bin.join("claude"),
        format!(
            "#!/bin/sh\nprintf '%s|%s\\n' \"$PWD\" \"$*\" >> '{}'\n",
            args_file.display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(bin.join("claude")).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(bin.join("claude"), perms).unwrap();

    let out = batch(
        &server,
        Some(&pane),
        &repo,
        &[
            "shape-batch",
            "--continue",
            "results load slowly, sometimes",
            "ABC-7",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    let ran = loop {
        let ran = std::fs::read_to_string(&args_file).unwrap_or_default();
        if ran.lines().count() >= 2 || Instant::now() > deadline {
            break ran;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut lines: Vec<&str> = ran.lines().collect();
    lines.sort();
    let at = repo.canonicalize().unwrap();
    assert_eq!(
        lines,
        vec![
            format!("{}|/shape ABC-7 --continue", at.display()),
            format!("{}|/shape results load slowly, sometimes", at.display()),
        ],
        "claude started in the repository with /shape for each"
    );
    let all = tabs(&server);
    assert_eq!(all.len(), 3);
    let labels: Vec<&str> = all[1..]
        .iter()
        .map(|t| t["label"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        labels,
        vec!["Shaping: results load slowly, sometimes", "Shaping: ABC-7"],
        "each named by shape, in the order given, at the end"
    );
}
