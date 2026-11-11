//! queue-and-clean's landing lock, `switchboard land-lock`, run as the skill
//! runs it, in worktrees of a temporary repository with a temporary HOME
//! whose Claude session files name this test's own process, so each
//! worktree has a live session: one worktree at a time holds the lock,
//! waiters take it in the queue's order, and a gone landing's lock or place
//! is taken over. And the landing's phase, set on a pane of a test server of
//! its own, and a landing made without asking, which marks its tab and shows
//! in the notifications. Ported from test/worktree-from-note.test.js's landing tests.

use crate::common;

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

/// `land-lock` run in `cwd` outside any pane or Claude session, as a
/// command to add to.
fn land_lock(home: &Path, cwd: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(BIN);
    command
        .arg("land-lock")
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("LAND_LOCK_POLL", "0.1")
        .env_remove("SWITCHBOARD")
        .env_remove("SWITCHBOARD_PANE")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .stdin(Stdio::null());
    command
}

fn lock(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    land_lock(home, cwd, args).output().unwrap()
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
fn a_landing_keeps_the_lock_while_the_session_that_took_it_runs_wherever_it_started() {
    let tmp = TempDir::new("land-lock-session");
    let home = tmp.path();
    let repo = repo(home);
    let one = home.join("one");
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "one", one.to_str().unwrap()],
    );
    let two = worktree(home, &repo, home, "two");
    // The landing's Claude started in the main clone and went into the
    // worktree to land it: no session file names the worktree.
    session(home, "lander", &repo);
    let in_session = |args: &[&str]| {
        land_lock(home, &one, args)
            .env("CLAUDE_CODE_SESSION_ID", "lander")
            .env("SWITCHBOARD_PANE", "%7")
            .output()
            .unwrap()
    };
    let holder_file = repo.join(".git/land.lock/holder");
    let queue = || stdout(&lock(home, &repo, &["queue"]));

    let read = |file: &Path| -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
    };

    assert_eq!(code(&in_session(&["take"])), 0);
    let holder = read(&holder_file);
    assert_eq!(
        (&holder["session"], &holder["pane"]),
        (&json!("lander"), &json!("%7")),
        "the holder names the session and pane that took it"
    );
    let held = lock(home, &two, &["take"]);
    assert_eq!(code(&held), 3, "its session keeps it, working elsewhere");
    assert!(
        stdout(&held).contains(&format!("held by {} since ", one.display())),
        "{}",
        stdout(&held)
    );

    // Taken up again by another session once the one that took it ended:
    // the holder names the new one, since kept.
    std::fs::write(
        &holder_file,
        json!({ "path": one, "since": 5, "session": "ended" }).to_string(),
    )
    .unwrap();
    assert_eq!(code(&in_session(&["take"])), 0);
    let holder = read(&holder_file);
    assert_eq!(
        (&holder["session"], &holder["since"]),
        (&json!("lander"), &json!(5))
    );
    assert_eq!(
        code(&lock(home, &two, &["take"])),
        3,
        "and keeps it while that one runs"
    );

    // Written by a build from before, with no session: read by its worktree
    // alone, as before.
    std::fs::write(&holder_file, json!({ "path": one, "since": 0 }).to_string()).unwrap();
    assert_eq!(code(&lock(home, &two, &["take"])), 0);

    // A place in the queue whose session ended, taken up by another
    // session, names it, the place ahead of the main clone's and since
    // kept.
    let queue_file = repo.join(".git/land.queue");
    std::fs::write(
        &queue_file,
        json!([
            { "path": one, "since": 3, "session": "ended" },
            { "path": repo, "since": 4 },
        ])
        .to_string(),
    )
    .unwrap();
    assert_eq!(code(&in_session(&["take"])), 3);
    let queued = read(&queue_file);
    assert_eq!(
        (
            &queued[0]["session"],
            &queued[0]["since"],
            &queued[1]["path"]
        ),
        (&json!("lander"), &json!(3), &json!(repo))
    );
    assert!(
        queue().contains(&format!("2nd\t{}\t", one.display())),
        "a waiter whose session works elsewhere stays in the queue: {}",
        queue()
    );
    std::fs::remove_file(home.join(".claude/sessions/lander.json")).unwrap();
    assert!(
        !queue().contains(&one.display().to_string()),
        "and leaves once its session ends: {}",
        queue()
    );

    std::fs::write(
        &holder_file,
        json!({ "path": one, "since": 0, "session": "lander" }).to_string(),
    )
    .unwrap();
    assert_eq!(
        code(&lock(home, &two, &["take"])),
        0,
        "a holder whose session ended has its lock taken over"
    );
}

#[test]
fn worktrees_landing_on_different_branches_hold_their_own_locks_at_once() {
    let tmp = TempDir::new("land-targets");
    let home = tmp.path();
    let repo = repo(home);
    git(&repo, &["branch", "team/next"]);
    let one = worktree(home, &repo, home, "one");
    let two = worktree(home, &repo, home, "two");
    let three = worktree(home, &repo, home, "three");
    let four = worktree(home, &repo, home, "four");
    // As wtn --target records it; naming the default branch is landing on it.
    git(&repo, &["config", "branch.two.wtnTarget", "team/next"]);
    git(&repo, &["config", "branch.four.wtnTarget", "main"]);

    assert_eq!(
        code(&lock(home, &one, &["take"])),
        0,
        "the default branch's"
    );
    assert_eq!(
        code(&lock(home, &two, &["take"])),
        0,
        "team/next's, taken while the default branch's is held"
    );
    assert!(repo.join(".git/land.lock/holder").exists());
    assert!(repo.join(".git/land.lock.team%2Fnext/holder").exists());

    let behind = lock(home, &three, &["--target", "team/next", "take"]);
    assert_eq!(
        code(&behind),
        3,
        "an explicit target waits on that branch's"
    );
    assert!(
        stdout(&behind).contains(&format!("held by {}", two.display())),
        "{}",
        stdout(&behind)
    );
    let places = |out: Output| -> Vec<String> {
        stdout(&out)
            .lines()
            .map(|l| l.split('\t').take(2).collect::<Vec<_>>().join("\t"))
            .collect()
    };
    assert_eq!(
        places(lock(home, &two, &["queue"])),
        vec![
            format!("landing\t{}", two.display()),
            format!("2nd\t{}", three.display())
        ]
    );
    assert_eq!(
        places(lock(home, &one, &["queue"])),
        vec![format!("landing\t{}", one.display())],
        "the default branch's queue has no one waiting"
    );
    assert_eq!(
        code(&lock(home, &four, &["take"])),
        3,
        "a target naming the default branch waits on its lock"
    );

    assert_eq!(code(&lock(home, &two, &["release"])), 0);
    assert_eq!(
        stdout(&lock(home, &one, &["holder"])).trim(),
        one.to_str().unwrap(),
        "letting team/next's go leaves the default branch's held"
    );
    assert_eq!(
        code(&lock(home, &three, &["--target", "team/next", "take"])),
        0
    );
}

#[test]
fn a_branch_named_lock_queues_apart_from_the_default_branch_s_mutex() {
    let tmp = TempDir::new("land-named-lock");
    let home = tmp.path();
    let repo = repo(home);
    git(&repo, &["branch", "lock"]);
    let one = worktree(home, &repo, home, "one");
    let two = worktree(home, &repo, home, "two");
    let three = worktree(home, &repo, home, "three");
    git(&repo, &["config", "branch.one.wtnTarget", "lock"]);
    git(&repo, &["config", "branch.two.wtnTarget", "lock"]);

    assert_eq!(code(&lock(home, &one, &["take"])), 0);
    assert_eq!(code(&lock(home, &two, &["take"])), 3, "waits on lock's");
    assert!(repo.join(".git/land.lock.lock/holder").exists());
    assert!(repo.join(".git/land.queue.lock").is_file());

    assert_eq!(
        code(&lock(home, &three, &["take"])),
        0,
        "the default branch's queue isn't locked by lock's queue"
    );
    assert!(!repo.join(".git/land.mutex").exists());
    assert!(repo.join(".git/land.queue.lock").is_file());
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

    // A waiter taken out of the queue from elsewhere (mission control's
    // Landing view) stops waiting rather than joining again.
    let waiting = land_lock(home, &two, &["wait"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    // Taken out until it stops: a waiter that starts after the first leave
    // joins again, as a new one does, and the next leave takes it out.
    let mut waiting = waiting;
    let deadline = common::deadline(Duration::from_secs(10));
    loop {
        assert_eq!(code(&lock(home, &two, &["leave"])), 0);
        let until = common::deadline(Duration::from_millis(500));
        while std::time::Instant::now() < until && waiting.try_wait().unwrap().is_none() {
            std::thread::sleep(Duration::from_millis(20));
        }
        if waiting.try_wait().unwrap().is_some() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "wait never stopped");
    }
    let waited = waiting.wait_with_output().unwrap();
    assert_eq!(code(&waited), 4, "wait exits 4");
    assert_eq!(stdout(&waited).trim(), "taken out of the landing queue");
    assert_eq!(paths(), vec![s(&three)], "and stays out");
    assert_eq!(code(&lock(home, &two, &["take"])), 3);

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

// test/tmux-sidebar.test.js's "a landing without asking marks its tab,
// shows in the notifications until dismissed, and its menu goes to its tab",
// with the Notifications window off, so Enter in the sidebar acts at once.
#[test]
fn a_landing_without_asking_marks_its_tab_and_shows_in_the_notifications_until_acted_on() {
    let server = TestServer::start();
    common::write_settings(
        &server.home,
        json!({ "tips": false, "notifications": false, "notice_list": false }),
    );
    let mut client = server.attach(140, 36);
    client.wait_for("a prompt", |s| s.iter().any(|l| l.contains('$')));
    let list = server.list();
    let tab = list["sessions"][0]["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let pane = list["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let inside = |args: &[&str]| {
        Command::new(BIN)
            .args(args)
            .current_dir(server.dir.path())
            .env("HOME", &server.home)
            .env("SWITCHBOARD_STATE_DIR", &server.state)
            .env("SWITCHBOARD", server.socket())
            .env("SWITCHBOARD_PANE", &pane)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    // The mode's switch on its tab, as queue-and-clean reads it from its
    // pane.
    server.ok(&["set", "-t", &tab, "@autopilot_land", "1"]);
    let shown = inside(&["show", "@autopilot_land"]);
    assert_eq!(String::from_utf8_lossy(&shown.stdout).trim(), "1");

    let said = inside(&["land-lock", "autolanded", "abc1234", "Make", "the", "thing"]);
    assert!(
        said.status.success(),
        "{}",
        String::from_utf8_lossy(&said.stderr)
    );
    assert_eq!(
        server
            .ok(&["show", "-t", &tab, "@autopilot_autoland"])
            .trim(),
        "abc1234",
        "its tab marked"
    );
    client.wait_text("Landed abc1234: Make the thing");

    // Said again by a hands-free tab, with what it left for you to apply:
    // the same notification, carrying it.
    let again = inside(&[
        "land-lock",
        "autolanded",
        "abc1234",
        "--not-applied",
        "Build Switchboard",
        "Make",
        "the",
        "thing",
    ]);
    assert!(again.status.success());
    let kept: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(server.state.join("autolanded.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(kept.as_array().unwrap().len(), 1, "one landing: {kept}");
    assert_eq!(kept[0]["notApplied"], "Build Switchboard");
    assert_eq!(kept[0]["item"], "Make the thing");

    // A second tab in front; Enter on the notification goes back to the
    // landing's tab and dismisses it.
    server.ok(&["new-tab"]);
    client.type_text("\x1bs");
    client.type_text("n");
    client.type_text("\r");
    let deadline = common::deadline(std::time::Duration::from_secs(20));
    while server.list()["sessions"][0]["tabs"][0]["current"] != true {
        assert!(
            std::time::Instant::now() < deadline,
            "the landing's tab in front"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    client.wait_for("the notification let go", |s| {
        !s.iter().any(|l| l.contains("Landed abc1234"))
    });
    let left = std::fs::read_to_string(server.state.join("autolanded.json")).unwrap();
    assert_eq!(left.trim(), "[]");
}

/// `switchboard landing`, as queue-and-clean asks it how a repository lands.
fn landing(home: &Path, dir: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .arg("landing")
        .args(args)
        .arg(dir)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("SWITCHBOARD")
        .env_remove("SWITCHBOARD_PANE")
        .env_remove("SWITCHBOARD_STATE_DIR")
        .env_remove("SWITCHBOARD_SETTINGS")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn landing_says_how_a_repository_lands_from_repos_json_else_through_a_pr_with_github_off() {
    let tmp = TempDir::new("landing");
    let home = tmp.path();
    let repo = repo(home);
    git(
        &repo,
        &["remote", "add", "origin", "git@github.com:acme/direct.git"],
    );
    let own = home.join(".config/switchboard");
    std::fs::create_dir_all(&own).unwrap();
    // Nothing asks GitHub or Claude here: the github setting is off.
    std::fs::write(own.join("settings.json"), r#"{"github": false}"#).unwrap();
    std::fs::write(
        own.join("repos.json"),
        r#"{"acme/direct": {"landing": "direct", "test": "npm test", "squash": false}}"#,
    )
    .unwrap();
    let out = landing(home, &repo, &[]);
    assert_eq!(code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let text = stdout(&out);
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("direct"));
    let steps: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(
        (
            &steps["test"],
            &steps["testIn"],
            &steps["squash"],
            &steps["review"],
            &steps["source"]
        ),
        (
            &json!("npm test"),
            &json!("here"),
            &json!(false),
            &json!("review-loop"),
            &json!("repos.json")
        )
    );
    let json_of =
        |out: Output| -> serde_json::Value { serde_json::from_str(&stdout(&out)).unwrap() };
    assert_eq!(
        json_of(landing(home, &repo, &["--json"]))["landing"],
        "direct"
    );

    std::fs::write(own.join("repos.json"), "{}").unwrap();
    let all = json_of(landing(home, &repo, &["--json"]));
    assert_eq!(all["landing"], "pr", "with GitHub not to be asked, a PR");
    assert!(
        all["why"]
            .as_str()
            .unwrap()
            .contains("github setting is off")
    );
    assert_eq!(
        all["test"],
        serde_json::Value::Null,
        "a test command not set"
    );

    std::fs::write(own.join("repos.json"), "{ broken").unwrap();
    let broken = landing(home, &repo, &[]);
    assert_eq!(code(&broken), 1, "a repos.json that can't be read stops it");
}

// Two tabs in one clone, the lock taken from a pane of the second: only that
// tab is landing, not every tab in the directory.
#[test]
fn only_the_tab_whose_pane_took_the_lock_is_landing_among_tabs_in_its_directory() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let repo = repo(server.dir.path());
    let dir = repo.to_str().unwrap();
    server.ok(&["new-tab", "-d", "-c", dir, "-n", "design"]);
    server.ok(&["new-tab", "-d", "-c", dir, "-n", "merge"]);
    let tabs = || server.list()["sessions"][0]["tabs"].clone();
    let pane = tabs()[2]["panes"][0]["id"].as_str().unwrap().to_string();
    session(&server.home, "lander", &repo);
    let took = land_lock(&server.home, &repo, &["take"])
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env("SWITCHBOARD_PANE", &pane)
        .env("CLAUDE_CODE_SESSION_ID", "lander")
        .output()
        .unwrap();
    assert_eq!(code(&took), 0, "{}", stdout(&took));

    let deadline = common::deadline(Duration::from_secs(20));
    let tabs = loop {
        let tabs = tabs();
        if !tabs[2]["facts"]["landing"].is_null() {
            break tabs;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the tab never landed: {tabs}"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(tabs[2]["facts"]["landing"]["holding"], true);
    assert_eq!(
        tabs[1]["facts"]["landing"],
        serde_json::Value::Null,
        "the other tab in the clone isn't landing"
    );
}
