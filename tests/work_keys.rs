//! The keys that start work (src/cmd/work.rs), pressed in a client of a
//! test server: M-t's directory list and the Claude tab it opens, M-W's
//! tickets each in a tab of its own running `wt`, M-B's branch made in a
//! worktree by `wtb`, and M-o's worktrees with no tab. A stand-in `claude`
//! in the test home's `bin` says where it ran, and Linear is an address
//! nothing answers on, so nothing leaves the machine.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use common::TestServer;
use serde_json::Value;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A server whose home has a repository with one commit and a stand-in
/// Claude, and whose current tab is in the repository.
fn world() -> (TestServer, PathBuf, common::TestClient) {
    let server = TestServer::start_with_env(&[
        ("LINEAR_API_KEY", "lin_test"),
        ("SWITCHBOARD_LINEAR_API", "http://127.0.0.1:9"),
    ]);
    let bin = server.home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let claude = bin.join("claude");
    std::fs::write(
        &claude,
        "#!/bin/sh\necho \"$PWD $*\" >> \"$HOME/claude-ran\"\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    let repo = server.home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    let repo = repo.canonicalize().unwrap();
    let mut client = server.attach(120, 40);
    client.wait_text("$");
    server.ok(&["new-tab", "-c", repo.to_str().unwrap()]);
    (server, repo, client)
}

fn tabs(server: &TestServer) -> Vec<Value> {
    server.list()["sessions"][0]["tabs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn wait(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn m_t_lists_the_directory_youre_in_first_and_opens_claude_in_the_one_picked() {
    let (server, repo, mut client) = world();
    let before = tabs(&server).len();
    client.send(b"\x1bt");
    let screen = client.wait_text("Open in >");
    let first = screen
        .iter()
        .position(|l| l.contains("~/repo"))
        .expect("the repository is listed");
    assert!(
        screen
            .iter()
            .position(|l| l.trim_start().contains("› ~"))
            .is_some_and(|at| at == first),
        "the directory you are in first, chosen: {screen:#?}"
    );
    client.send(b"\r");
    wait("Claude to run in the new tab", || {
        std::fs::read_to_string(server.home.join("claude-ran"))
            .is_ok_and(|t| t.starts_with(repo.to_str().unwrap()))
    });
    let all = tabs(&server);
    assert_eq!(all.len(), before + 1);
    assert_eq!(all.last().unwrap()["current"], true, "at the end, in front");
    // Then what to call it: s, shaping a ticket, asks which, since the
    // repository's branch names none.
    client.wait_text("Name the tab");
    client.type_text("s");
    client.wait_text("Its ID, like ABC-123");
    client.type_text("abc-7\r");
    wait("the tab named", || {
        tabs(&server).last().unwrap()["options"]["@tab_name"] == "Shaping: ABC-7"
    });
}

#[test]
fn m_shift_w_opens_a_tab_for_each_ticket_typed_running_wt_and_refuses_anything_else() {
    let (server, _repo, mut client) = world();
    let before = tabs(&server).len();
    client.send(b"\x1bW");
    client.wait_text("Open in >");
    client.send(b"\r");
    client.wait_text("Tickets:");
    client.type_text("abc-1, ABC-2 abc-1");
    client.send(b"\r");
    wait("a tab for each ticket", || {
        tabs(&server).len() == before + 2
    });
    let all = tabs(&server);
    let opened: Vec<String> = all[before..]
        .iter()
        .map(|t| t["panes"][0]["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        all[before]["current"], true,
        "the first brought to the front"
    );
    // wt ran in each, defined from worktree-from-ticket's init, and asked
    // Linear, which isn't there.
    for pane in &opened {
        wait("wt to fail on Linear", || {
            server.ok(&["capture-pane", "-t", pane]).contains("Error:")
        });
    }

    client.send(b"\x1bW");
    client.wait_text("Open in >");
    client.send(b"\r");
    client.wait_text("Tickets:");
    client.type_text("ABC-3 $(touch x)");
    client.send(b"\r");
    client.wait_text("Not a ticket ID or Linear issue URL: $(touch");
    assert_eq!(tabs(&server).len(), before + 2, "no tab half-started");
}

#[test]
fn m_shift_b_makes_a_branch_no_branch_matches_in_a_worktree_and_starts_claude_there() {
    let (server, repo, mut client) = world();
    client.send(b"\x1bB");
    client.wait_text("Open in >");
    client.send(b"\r");
    client.wait_text("branch>");
    client.type_text("fix-the-login");
    client.send(b"\r");
    let worktree = repo.join(".claude/worktrees/fix-the-login");
    wait("Claude in the branch's worktree", || {
        std::fs::read_to_string(server.home.join("claude-ran"))
            .is_ok_and(|t| t.contains(worktree.to_str().unwrap()))
    });
}

#[test]
fn m_o_lists_a_worktree_no_tab_has_open_and_reopens_it_in_a_tab() {
    let (server, repo, mut client) = world();
    let tree = repo.join(".claude/worktrees/lonely");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "lonely",
            tree.to_str().unwrap(),
        ],
    );
    // M-o opens mission control's Work map as a picker of the work no tab
    // has open, a row each.
    client.send(b"\x1bo");
    client.wait_for("the worktree's row", |s| {
        s.iter()
            .any(|l| l.contains("lonely") && l.contains("no tab"))
    });
    // Enter opens its menu, as the daemon's picker did: a tab there first,
    // then its diff and lazygit, and its directory to copy.
    client.send(b"\r");
    let screen = client.wait_text("Open a tab there, resuming its Claude");
    for offered in ["Its diff (M-d)", "lazygit", "Copy its directory"] {
        assert!(
            screen.iter().any(|l| l.contains(offered)),
            "{offered}: {screen:#?}"
        );
    }
    client.send(b"\r");
    wait("a tab in the worktree", || {
        tabs(&server).iter().any(|t| {
            t["panes"][0]["cwd"]
                .as_str()
                .is_some_and(|c| Path::new(c) == tree)
        })
    });
}

#[test]
fn a_key_of_a_service_turned_off_shows_its_tip_once_and_does_nothing() {
    let (server, _repo, mut client) = world();
    let config = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("settings.json"), r#"{"linear": false}"#).unwrap();
    // The server reads the file again on its next tick.
    std::thread::sleep(Duration::from_millis(2500));
    client.send(b"\x1bW");
    client.wait_text("Linear is off in Settings.");
    // Any key closes the tip and goes on; M-W again does nothing at all.
    client.send(b"\x1bW");
    client.wait_for("the tip to close", |s| {
        !s.iter().any(|l| l.contains("Linear is off"))
    });
    std::thread::sleep(Duration::from_millis(500));
    let screen = client.screen();
    assert!(
        !screen
            .iter()
            .any(|l| l.contains("Open in >") || l.contains("Linear is off")),
        "{screen:#?}"
    );
}
