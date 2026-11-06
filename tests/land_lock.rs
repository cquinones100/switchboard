//! queue-and-clean's landing lock, `switchboard land-lock`, run as the skill
//! runs it, in worktrees of a temporary repository with a temporary HOME
//! whose Claude session files name this test's own process, so each
//! worktree has a live session: one worktree at a time holds the lock,
//! waiters take it in the queue's order, and a gone landing's lock or place
//! is taken over. And the landing's phase, set on a pane of a test server of
//! its own. Ported from test/worktree-from-note.test.js's landing tests.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use common::{BIN, TempDir, TestServer};
use serde_json::json;

/// A repository with a commit, in `tmp/repo`.
fn repo(tmp: &Path) -> PathBuf {
    let repo = tmp.join("repo");
    git(tmp, &["init", "-q", repo.to_str().unwrap()]);
    git(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    repo
}

fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A worktree of `repo` on a branch of its own, with a Claude session in it.
fn worktree(home: &Path, repo: &Path, at: &Path, name: &str) -> PathBuf {
    let dir = at.join(name);
    git(
        repo,
        &["worktree", "add", "-q", "-b", name, dir.to_str().unwrap()],
    );
    session(home, name, &dir);
    dir
}

/// A running Claude session in `dir`, as Claude Code records one: this
/// test's own process stands in for it.
fn session(home: &Path, name: &str, dir: &Path) {
    let sessions = home.join(".claude/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join(format!("{name}.json")),
        json!({ "sessionId": name, "cwd": dir, "pid": std::process::id() }).to_string(),
    )
    .unwrap();
}

fn lock(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .arg("land-lock")
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("LAND_LOCK_POLL", "0.1")
        .env_remove("SWITCHBOARD")
        .env_remove("SWITCHBOARD_PANE")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn one_worktree_at_a_time_the_second_taking_it_once_the_first_lets_go_and_a_dead_landing_taken_over()
 {
    let tmp = TempDir::new("land-lock");
    let home = tmp.path();
    let repo = repo(home);
    let one = worktree(home, &repo, home, "one");
    let two = worktree(home, &repo, home, "two");

    assert_eq!(code(&lock(home, &one, &["take"])), 0, "the first takes it");
    assert_eq!(
        code(&lock(home, &one, &["take"])),
        0,
        "and taking it again is fine"
    );
    let held = lock(home, &two, &["take"]);
    assert_eq!(code(&held), 3, "the second is told to wait");
    assert!(
        stdout(&held).contains(&format!("held by {} since ", one.display())),
        "{}",
        stdout(&held)
    );
    assert_eq!(code(&lock(home, &two, &["release"])), 0);
    assert_eq!(
        stdout(&lock(home, &two, &["holder"])).trim(),
        one.to_str().unwrap(),
        "a release by another worktree lets nothing go"
    );
    assert_eq!(code(&lock(home, &one, &["release"])), 0);
    assert_eq!(
        code(&lock(home, &two, &["take"])),
        0,
        "the second takes it once the first lets go"
    );

    // Held for days while its push question waits: still held.
    std::fs::write(
        repo.join(".git/land.lock/holder"),
        json!({ "path": two, "since": 0 }).to_string(),
    )
    .unwrap();
    assert_eq!(
        code(&lock(home, &one, &["take"])),
        3,
        "a landing waiting on you keeps it however long"
    );

    // The second's session ends before it lets go: the first takes over.
    std::fs::remove_file(home.join(".claude/sessions/two.json")).unwrap();
    assert_eq!(
        code(&lock(home, &one, &["wait"])),
        0,
        "a landing whose session is gone has its lock taken over"
    );
    assert_eq!(
        stdout(&lock(home, &one, &["holder"])).trim(),
        one.to_str().unwrap()
    );

    // The first's worktree goes before it lets go: the second takes over.
    session(home, "two", &two);
    git(
        &repo,
        &["worktree", "remove", "--force", one.to_str().unwrap()],
    );
    assert_eq!(
        code(&lock(home, &two, &["take"])),
        0,
        "so does one whose worktree is gone"
    );
}

#[test]
fn waiters_take_it_in_the_queue_s_order_which_a_take_cant_jump_and_move_reorders() {
    let tmp = TempDir::new("land-queue");
    let home = tmp.path();
    let repo = repo(home);
    let one = worktree(home, &repo, home, "one");
    let two = worktree(home, &repo, home, "two");
    let three = worktree(home, &repo, home, "three");
    let queue = || -> Vec<(String, String)> {
        stdout(&lock(home, &repo, &["queue"]))
            .lines()
            .map(|l| {
                let mut f = l.split('\t');
                (
                    f.next().unwrap_or("").to_string(),
                    f.next().unwrap_or("").to_string(),
                )
            })
            .collect()
    };
    let paths = || -> Vec<String> { queue().into_iter().map(|(_, p)| p).collect() };
    let s = |p: &Path| p.to_str().unwrap().to_string();

    assert_eq!(code(&lock(home, &one, &["take"])), 0);
    let second = lock(home, &two, &["take"]);
    assert_eq!(code(&second), 3);
    assert!(stdout(&second).contains(&format!("held by {}", one.display())));
    assert_eq!(code(&lock(home, &three, &["take"])), 3);
    assert_eq!(
        queue(),
        vec![
            ("landing".into(), s(&one)),
            ("2nd".into(), s(&two)),
            ("3rd".into(), s(&three))
        ],
        "each that was told to wait joined the queue, in order"
    );
    let line = stdout(&lock(home, &repo, &["queue"]));
    assert!(
        line.lines().all(|l| l
            .split('\t')
            .nth(2)
            .is_some_and(|t| t.parse::<i64>().is_ok())),
        "each with when it joined: {line}"
    );

    assert_eq!(
        code(&lock(home, &repo, &["move", three.to_str().unwrap(), "up"])),
        0
    );
    assert_eq!(
        paths(),
        vec![s(&one), s(&three), s(&two)],
        "move up swaps them"
    );
    assert_eq!(
        code(&lock(home, &repo, &["move", one.to_str().unwrap(), "up"])),
        3,
        "the landing one isn't in the queue to move"
    );
    assert_eq!(
        code(&lock(home, &repo, &["move", "x", "sideways"])),
        2,
        "a way that isn't one is a usage error"
    );

    lock(home, &one, &["release"]);
    let jumped = lock(home, &two, &["take"]);
    assert_eq!(
        code(&jumped),
        3,
        "a take behind another waiter waits, with the lock free"
    );
    assert!(stdout(&jumped).contains("1 ahead in the landing queue"));
    assert_eq!(
        code(&lock(home, &three, &["wait"])),
        0,
        "the first in line takes it"
    );
    assert_eq!(
        queue(),
        vec![("landing".into(), s(&three)), ("2nd".into(), s(&two))],
        "and leaves the queue"
    );

    lock(home, &two, &["release"]);
    assert_eq!(
        queue(),
        vec![("landing".into(), s(&three))],
        "a waiter stopped and released leaves the queue, the lock still the landing one's"
    );
    assert_eq!(code(&lock(home, &two, &["take"])), 3);
    assert_eq!(code(&lock(home, &one, &["take"])), 3);
    assert_eq!(paths(), vec![s(&three), s(&two), s(&one)]);
    assert_eq!(
        code(&lock(
            home,
            &repo,
            &["move", one.to_str().unwrap(), "first"]
        )),
        0
    );
    assert_eq!(
        paths(),
        vec![s(&three), s(&one), s(&two)],
        "first puts it at the head"
    );
    std::fs::remove_file(home.join(".claude/sessions/one.json")).unwrap();
    assert_eq!(
        paths(),
        vec![s(&three), s(&two)],
        "a waiter whose session is gone is dropped"
    );

    // The main clone queued with no session of its own: one in a worktree
    // nested inside it doesn't keep it.
    let nested = repo.join(".claude/worktrees/nested");
    std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "nested",
            nested.to_str().unwrap(),
        ],
    );
    session(home, "nested", &nested);
    std::fs::write(
        repo.join(".git/land.queue"),
        json!([{ "path": repo, "since": 1 }, { "path": two, "since": 2 }]).to_string(),
    )
    .unwrap();
    assert_eq!(
        paths(),
        vec![s(&three), s(&two)],
        "a session in a nested worktree is that worktree's"
    );

    let outside = lock(home, Path::new("/"), &["take"]);
    assert_eq!(code(&outside), 2, "outside a repository it says so");
}

#[test]
fn the_landing_s_phase_marks_the_pane_with_its_step_and_queue_and_clean_s_loop() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_for("a prompt", |s| s.iter().any(|l| l.contains('$')));
    let list = server.list();
    let pane = list["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let repo = repo(server.dir.path());
    let phase = |cwd: &Path, words: &[&str], inside: bool| {
        let mut cmd = Command::new(BIN);
        cmd.arg("land-lock")
            .arg("phase")
            .args(words)
            .current_dir(cwd)
            .env("HOME", &server.home)
            .env("SWITCHBOARD_STATE_DIR", &server.state)
            .env_remove("SWITCHBOARD")
            .env_remove("SWITCHBOARD_PANE")
            .stdin(Stdio::null());
        if inside {
            cmd.env("SWITCHBOARD", server.socket())
                .env("SWITCHBOARD_PANE", &pane);
        }
        cmd.status().unwrap().code()
    };
    let shown = || {
        let p = server.ok(&["show", "-t", &pane, "@agent_phase"]);
        let l = server.ok(&["show", "-t", &pane, "@agent_loop"]);
        format!("{}|{}", p.trim(), l.trim())
    };

    assert_eq!(
        phase(&repo, &["waiting", "for", "the", "landing  lock"], true),
        Some(0)
    );
    assert_eq!(shown(), "waiting for the landing lock|queue");
    // Cleared last, after the cleanup has removed the worktree: from outside
    // a repository.
    assert_eq!(phase(server.dir.path(), &[], true), Some(0));
    assert_eq!(shown(), "|queue", "the step cleared, the loop kept");
    assert_eq!(
        phase(&repo, &["testing"], false),
        Some(0),
        "outside Switchboard it does nothing"
    );
    assert_eq!(shown(), "|queue");
    std::thread::sleep(Duration::from_millis(10));
}
