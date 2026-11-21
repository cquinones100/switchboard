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
    // As wt --target records it; naming the default branch is landing on it.
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
        json!({ "tips": false, "notifications": false, "notice_list": false, "autopilot_landing_notices": true }),
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
    let inside_at = |dir: &Path, args: &[&str]| {
        Command::new(BIN)
            .args(args)
            .current_dir(dir)
            .env("HOME", &server.home)
            .env("SWITCHBOARD_STATE_DIR", &server.state)
            .env("SWITCHBOARD", server.socket())
            .env("SWITCHBOARD_PANE", &pane)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let inside = |args: &[&str]| inside_at(server.dir.path(), args);
    let notices = relay_stand_in(&server.home);
    // The mode's switch on its tab, as queue-and-clean reads it from its
    // pane.
    server.ok(&["set", "-t", &tab, "@land", "1"]);
    let shown = inside(&["show", "@land"]);
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
    assert!(
        notices.lock().unwrap().is_empty(),
        "only the landing's push sends the Slack notice"
    );

    // Said again by a hands-free tab, with the range the main clone moved
    // by: the same notification, carrying what that range needs, the item
    // kept.
    let repo = repo(server.dir.path());
    std::fs::write(repo.join(".env.example"), "A=1\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "env"]);
    let again = inside_at(
        &repo,
        &[
            "land-lock",
            "autolanded",
            "abc1234",
            "--apply-range",
            "HEAD~1",
            "HEAD",
        ],
    );
    assert!(
        again.status.success(),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    let kept: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(server.state.join("autolanded.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(kept.as_array().unwrap().len(), 1, "one landing: {kept}");
    assert_eq!(
        kept[0]["notApplied"],
        "Copy what's new in .env.example into your .env"
    );
    assert_eq!(kept[0]["item"], "Make the thing");
    // A range that needs nothing clears what was left.
    let empty = inside_at(
        &repo,
        &[
            "land-lock",
            "autolanded",
            "abc1234",
            "--apply-range",
            "HEAD",
            "HEAD",
        ],
    );
    assert!(empty.status.success());
    let kept: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(server.state.join("autolanded.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(kept[0]["notApplied"], "");
    assert_eq!(kept[0]["item"], "Make the thing");
    // A range that is an option never reaches git.
    let written = server.dir.path().join("written");
    let option = format!("--output={}", written.display());
    let refused = inside_at(
        &repo,
        &[
            "land-lock",
            "autolanded",
            "abc1234",
            "--apply-range",
            &option,
            "HEAD",
        ],
    );
    assert!(!refused.status.success());
    assert!(!written.exists(), "git wrote no file");

    // A second tab in front; Enter on the notification opens mission
    // control's Notifications view with its menu over it, which goes back
    // to the landing's tab first and dismisses it.
    server.ok(&["new-tab"]);
    client.type_text("\x1bs");
    client.type_text("n");
    client.type_text("\r");
    client.wait_text("Go to its tab");
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

// With "Notify when autopilot lands" off, as it starts, a landing made
// without asking still marks its tab, but goes straight into the
// notifications' history as dismissed and never shows in the sidebar.
#[test]
fn a_landing_without_asking_stays_out_of_the_sidebar_with_its_notifications_off() {
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
    let notices = relay_stand_in(&server.home);
    let said = Command::new(BIN)
        .args(["land-lock", "autolanded", "abc1234", "Make", "the", "thing"])
        .current_dir(server.dir.path())
        .env("HOME", &server.home)
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env("SWITCHBOARD", server.socket())
        .env("SWITCHBOARD_PANE", &pane)
        .stdin(Stdio::null())
        .output()
        .unwrap();
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
    assert!(
        notices.lock().unwrap().is_empty(),
        "nothing sent to Slack with the setting off"
    );
    let history = server.state.join("notifications");
    let dismissed = || {
        std::fs::read_dir(&history)
            .ok()
            .into_iter()
            .flatten()
            .any(|day| {
                let Ok(text) = std::fs::read_to_string(day.unwrap().path()) else {
                    return false;
                };
                let entries: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                entries.as_array().into_iter().flatten().any(|e| {
                    e["source"] == "autolanded"
                        && e["id"] == "abc1234"
                        && e["sidebar"] == "dismissed"
                })
            })
    };
    let deadline = common::deadline(Duration::from_secs(20));
    while !dismissed() {
        assert!(
            std::time::Instant::now() < deadline,
            "the landing kept in the history as dismissed"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let left = std::fs::read_to_string(server.state.join("autolanded.json")).unwrap();
    assert_eq!(left.trim(), "[]", "put away");
    let screen = client.screen();
    assert!(
        !screen.iter().any(|l| l.contains("Landed abc1234")),
        "not in the sidebar: {}",
        screen.join("\n")
    );
}

/// The Slack relay, as `slack-relay notify` finds it: its lock naming this
/// test's process, and a thread answering each notice queued in its outbox
/// as posted. The notices' words, as they were queued.
fn relay_stand_in(home: &Path) -> std::sync::Arc<std::sync::Mutex<Vec<String>>> {
    let state = home.join(".local/state/slack-relay");
    let outbox = state.join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    std::fs::write(
        state.join("relay.lock"),
        format!("{} test", std::process::id()),
    )
    .unwrap();
    let notices = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = notices.clone();
    std::thread::spawn(move || {
        loop {
            let Ok(entries) = std::fs::read_dir(&outbox) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|e| e != "json") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let notice: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                seen.lock()
                    .unwrap()
                    .push(notice["text"].as_str().unwrap_or_default().to_string());
                let _ = std::fs::remove_file(&path);
                let _ = std::fs::write(path.with_extension("sent"), "1.2");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    notices
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
        .env_remove("NOTES_DIR")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

// A branch `wt` opened for a note's item: `landing --json` names the
// default branch, the note and the item, the landing's CI branch, and
// whether the change is already on the default branch, which a squash of it
// landed there by other means makes it.
#[test]
fn landing_names_the_default_branch_note_item_and_ci_branch_and_whether_it_has_landed() {
    let tmp = TempDir::new("landing-names");
    let home = tmp.path();
    let repo = repo(home);
    let origin = home.join("origin.git");
    git(home, &["init", "-q", "--bare", origin.to_str().unwrap()]);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "origin", "main"]);
    git(&repo, &["fetch", "-q", "origin"]);
    git(&repo, &["remote", "set-head", "origin", "main"]);
    let own = home.join(".config/switchboard");
    std::fs::create_dir_all(&own).unwrap();
    std::fs::write(own.join("settings.json"), r#"{"github": false}"#).unwrap();
    std::fs::write(own.join("repos.json"), "{}").unwrap();
    let notes = home.join("Notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("Work.md"),
        "# Work\n\n- [ ] FEAT: Do the thing\n\n<!-- note-id: n-0123456789ab -->\n",
    )
    .unwrap();
    let tree = home.join("tree");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "note/do-the-thing",
            tree.to_str().unwrap(),
        ],
    );
    git(
        &repo,
        &[
            "config",
            "branch.note/do-the-thing.wtnItem",
            "n-0123456789ab: FEAT: Do the thing",
        ],
    );
    std::fs::write(tree.join("thing.txt"), "done\n").unwrap();
    git(&tree, &["add", "thing.txt"]);
    git(&tree, &["commit", "-q", "-m", "Do the thing"]);
    let json_of = || -> serde_json::Value {
        let out = landing(home, &tree, &["--json"]);
        assert_eq!(code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_str(&stdout(&out)).unwrap()
    };

    let all = json_of();
    assert_eq!(all["defaultBranch"], "main");
    assert!(
        all["note"].as_str().unwrap().ends_with("Notes/Work.md"),
        "{}",
        all["note"]
    );
    assert_eq!(all["item"], "FEAT: Do the thing");
    let ci = all["ciBranch"].as_str().unwrap();
    let id = ci.strip_prefix("ci/land-").unwrap();
    assert!(
        id.len() == 12 && id.chars().all(|c| c.is_ascii_hexdigit()),
        "{ci}"
    );
    assert_eq!(all["alreadyLanded"], false, "main lacks the change");

    // The change landed on main as a commit of its own, as a squash does.
    std::fs::write(repo.join("thing.txt"), "done\n").unwrap();
    git(&repo, &["add", "thing.txt"]);
    git(&repo, &["commit", "-q", "-m", "Squashed: do the thing"]);
    git(&repo, &["push", "-q", "origin", "main"]);
    git(&repo, &["fetch", "-q", "origin"]);
    assert_eq!(json_of()["alreadyLanded"], true);

    // A branch `wt` didn't open names no note or item.
    let plain = landing(home, &repo, &["--json"]);
    let plain: serde_json::Value = serde_json::from_str(&stdout(&plain)).unwrap();
    assert_eq!(
        (&plain["note"], &plain["item"]),
        (&serde_json::Value::Null, &serde_json::Value::Null)
    );
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

/// The option `label` of the menu open, reached by moving down to it, and
/// taken.
fn pick(client: &mut common::TestClient, label: &str) {
    for _ in 0..20 {
        let screen = client.wait_text(label);
        if screen.iter().any(|l| l.contains('›') && l.contains(label)) {
            client.type_text("\r");
            return;
        }
        let before = screen.join("\n");
        client.type_text("j");
        client.wait_for("the cursor to move", |s| s.join("\n") != before);
    }
    panic!("{label} not reached");
}

#[test]
fn with_ux2_the_landing_views_menu_moves_the_chosen_waiter_down_the_queue() {
    let server = TestServer::start();
    common::write_settings(
        &server.home,
        json!({ "ux2": true, "github": false, "linear": false, "sidebar": false, "tips": false }),
    );
    let mut client = server.attach(170, 40);
    client.wait_text("$");
    let repo = repo(server.dir.path());
    let at = server.dir.path();
    let [one, two, three] =
        ["one", "two", "three"].map(|name| worktree(&server.home, &repo, at, name));
    // The queue is whole before the tabs open, so no read of it the tabs
    // set off finds two last in a queue three hasn't joined yet.
    assert_eq!(code(&lock(&server.home, &one, &["take"])), 0);
    assert_eq!(code(&lock(&server.home, &two, &["take"])), 3);
    assert_eq!(code(&lock(&server.home, &three, &["take"])), 3);
    for (dir, name) in [(&one, "one"), (&two, "two"), (&three, "three")] {
        server.ok(&["new-tab", "-d", "-c", dir.to_str().unwrap(), "-n", name]);
    }
    let paths = || -> Vec<String> {
        stdout(&lock(&server.home, &repo, &["queue"]))
            .lines()
            .filter_map(|l| l.split('\t').nth(1).map(str::to_string))
            .collect()
    };
    let s = |p: &Path| p.to_str().unwrap().to_string();
    assert_eq!(paths(), vec![s(&one), s(&two), s(&three)]);

    server.ok(&["press", "mission-landing"]);
    client.wait_text("straight on the default branch");
    // The landing one is chosen first; one step down is the first waiter,
    // whose detail says it takes the lock at its turn. Each key waits for
    // the screen to show it, so none moves past it.
    client.wait_text("▌ one");
    client.type_text("j");
    client.wait_text("▌ two");
    client.wait_text("takes the lock at its turn");
    // J is no key of the view's: Enter's menu moves it.
    client.type_text("J");
    client.type_text("\r");
    pick(&mut client, "Move it down in the landing queue");
    let deadline = common::deadline(Duration::from_secs(20));
    while paths() != vec![s(&one), s(&three), s(&two)] {
        assert!(
            std::time::Instant::now() < deadline,
            "its menu never moved it down: {:?}",
            paths()
        );
        std::thread::sleep(Duration::from_millis(100) * common::patience());
    }
    // The cursor follows the moved waiter, so its menu moves the same one
    // back up, rather than the one now where it was, which is first and
    // stays, once the view has read the new order.
    client.wait_for("three listed before two", |screen| {
        let at = |name: &str| screen.iter().position(|l| l.contains(name));
        matches!((at("three"), at("▌ two")), (Some(t), Some(w)) if t < w)
    });
    client.type_text("\r");
    pick(&mut client, "Move it up in the landing queue");
    let deadline = common::deadline(Duration::from_secs(20));
    while paths() != vec![s(&one), s(&two), s(&three)] {
        assert!(
            std::time::Instant::now() < deadline,
            "its menu never moved it back up: {:?}",
            paths()
        );
        std::thread::sleep(Duration::from_millis(100) * common::patience());
    }
}

// A landing on the default branch opens a tab running update-target for
// each branch items land on: autopilot's target, and a hand-started item's
// `wtnTarget` while its worktree is there. Once each while it runs (here
// waiting on the targets' locks), again once it has ended, and none with the
// setting off.
#[test]
fn a_landing_on_the_default_branch_opens_a_tab_bringing_each_target_up_to_date() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let repo = repo(server.dir.path());
    let origin = server.dir.path().join("origin.git");
    git(
        server.dir.path(),
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(
        &repo,
        &["push", "-q", "origin", "main", "main:ux", "main:team/next"],
    );
    let item = worktree(&server.home, &repo, server.dir.path(), "item");
    git(&repo, &["config", "branch.item.wtnTarget", "team/next"]);
    // A landing onto each target holds its lock meanwhile.
    let held = |args: &[&str]| {
        land_lock(&server.home, &item, args)
            .env("CLAUDE_CODE_SESSION_ID", "item")
            .output()
            .unwrap()
    };
    for target in ["ux", "team/next"] {
        assert_eq!(code(&held(&["--target", target, "take"])), 0);
    }
    std::fs::write(
        server.state.join("autopilot.json"),
        json!({ "on": true, "repo": repo, "target": "ux" }).to_string(),
    )
    .unwrap();
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let landed = |branch: &str| {
        let out = land_lock(&server.home, &item, &["landed", "abc1234", branch])
            .env("SWITCHBOARD_STATE_DIR", &server.state)
            .env("SWITCHBOARD", server.socket())
            .env("SWITCHBOARD_PANE", &pane)
            .output()
            .unwrap();
        assert_eq!(code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    };
    let marks = || -> Vec<String> {
        let mut marks: Vec<String> = server.list()["sessions"][0]["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["options"]["@tab_update_target"].as_str())
            .map(|m| m.split(' ').next().unwrap().to_string())
            .collect();
        marks.sort();
        marks
    };

    common::write_settings(&server.home, json!({ "target_follows_default": false }));
    landed("main");
    assert!(marks().is_empty(), "the setting off opens none");

    common::write_settings(&server.home, json!({}));
    landed("ux");
    assert!(marks().is_empty(), "a landing on a target opens none");
    landed("main");
    assert_eq!(marks(), vec!["team/next".to_string(), "ux".to_string()]);
    let tabs = server.list()["sessions"][0]["tabs"].clone();
    let ux = tabs
        .as_array()
        .unwrap()
        .iter()
        .find(|t| {
            t["options"]["@tab_update_target"]
                .as_str()
                .is_some_and(|m| m.starts_with("ux "))
        })
        .unwrap();
    assert_eq!(ux["options"]["@tab_name"], "ux ← main");
    assert_eq!(ux["current"], false, "opened in the background");

    landed("main");
    assert_eq!(
        marks().len(),
        2,
        "a second landing while they run opens none"
    );

    for target in ["ux", "team/next"] {
        assert_eq!(code(&held(&["--target", target, "release"])), 0);
    }
    let deadline = common::deadline(Duration::from_secs(30));
    while !marks().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the runs never ended: {:?}",
            marks()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    landed("main");
    assert_eq!(
        marks().len(),
        2,
        "once they ended, the next landing opens them again"
    );
}

// M-p's "Bring a target branch up to date with the default branch" lists
// the pane's repository's branches but its default one.
#[test]
fn the_palette_command_picks_a_branch_other_than_the_default_one() {
    let server = TestServer::start();
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    let repo = repo(server.dir.path());
    git(&repo, &["branch", "ux"]);
    server.ok(&["new-tab", "-c", repo.to_str().unwrap()]);
    server.ok(&["press", "update-target"]);
    client.wait_text("Which branch takes main's work?");
    client.wait_text("ux");
}

/// A repository landing straight on main, pushed to a bare `origin`, with a
/// worktree `one` holding a commit to land, the landing lock taken and a
/// clean review recorded for it; and a test server whose first pane the
/// push runs from, as a tab's Claude runs it.
struct Landing {
    server: TestServer,
    _client: common::TestClient,
    origin: PathBuf,
    repo: PathBuf,
    one: PathBuf,
    tab: String,
    pane: String,
}

impl Landing {
    fn new() -> Landing {
        let server = TestServer::start();
        common::write_settings(
            &server.home,
            json!({ "tips": false, "github": false, "autopilot_landing_notices": false, "target_follows_default": false }),
        );
        let home = server.home.clone();
        let origin = home.join("origin.git");
        git(&home, &["init", "-q", "--bare", origin.to_str().unwrap()]);
        let repo = repo(&home).canonicalize().unwrap();
        git(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(&repo, &["push", "-q", "origin", "main"]);
        git(&repo, &["remote", "set-head", "origin", "main"]);
        std::fs::write(
            home.join(".config/switchboard/repos.json"),
            json!({ repo.join(".git").to_str().unwrap(): { "landing": "direct", "test": "true", "review": "review-loop" } })
                .to_string(),
        )
        .unwrap();
        let one = home.join("one");
        git(
            &repo,
            &["worktree", "add", "-q", "-b", "one", one.to_str().unwrap()],
        );
        let one = one.canonicalize().unwrap();
        std::fs::write(one.join("feature.txt"), "a feature\n").unwrap();
        git(&one, &["add", "feature.txt"]);
        git(&one, &["commit", "-q", "-m", "Add the feature"]);
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
        let landing = Landing {
            server,
            _client: client,
            origin,
            repo,
            one,
            tab,
            pane,
        };
        let taken = landing.run(&["land-lock", "take"]);
        assert_eq!(code(&taken), 0, "{}", stdout(&taken));
        landing.record_review();
        let tested = landing.run(&["land-lock", "record", "tests", r#"{"ok":true}"#]);
        assert_eq!(code(&tested), 0);
        landing
    }

    /// This program run in the worktree from the test server's pane.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(BIN)
            .args(args)
            .current_dir(&self.one)
            .env("HOME", &self.server.home)
            .env("SWITCHBOARD_STATE_DIR", &self.server.state)
            .env("SWITCHBOARD", self.server.socket())
            .env("SWITCHBOARD_PANE", &self.pane)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@e")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@e")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    /// Two clean rounds of the worktree's change, the second confirming, by
    /// a stand-in Claude, which codex-review records.
    fn record_review(&self) {
        let bin = self.server.home.join("bin");
        common::stand_in::Claude::new()
            .answer(
                r#"cat >/dev/null; echo '{"structured_output":{"summary":"ok","findings":[]}}'"#,
            )
            .install(&bin);
        for args in [&[][..], &["--include-low"][..]] {
            let out = Command::new(BIN)
                .args(["codex-review", "--reviewer", "claude", "--no-rules"])
                .args(args)
                .current_dir(&self.one)
                .env("HOME", &self.server.home)
                .env("SWITCHBOARD_STATE_DIR", &self.server.state)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("PATH", common::stand_in::path_with_gh(&bin))
                .env_remove("SWITCHBOARD")
                .env_remove("SWITCHBOARD_PANE")
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let recorded = self.run(&["codex-review", "--recorded"]);
        assert_eq!(
            code(&recorded),
            0,
            "{}",
            String::from_utf8_lossy(&recorded.stderr)
        );
    }

    fn on_origin(&self) -> String {
        rev(&self.origin, "main")
    }

    fn holder(&self) -> String {
        stdout(&self.run(&["land-lock", "holder"]))
            .trim()
            .to_string()
    }
}

fn rev(dir: &Path, what: &str) -> String {
    let out = Command::new("git")
        .args(["rev-parse", what])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn the_landing_push_lands_without_asking_through_its_gate_and_does_the_bookkeeping() {
    let l = Landing::new();
    let before = l.on_origin();
    let clone_before = rev(&l.repo, "HEAD");

    // Asking before landing: nothing pushed or run, the lock kept for the
    // question.
    std::fs::write(
        l.server.home.join(".config/switchboard/repos.json"),
        json!({ l.repo.join(".git").to_str().unwrap(): { "landing": "direct", "test": "true", "review": "review-loop", "beforePush": "touch \"$HOME/before-ran\"" } })
            .to_string(),
    )
    .unwrap();
    std::fs::write(
        l.server.home.join(".config/switchboard/trusted-dirs"),
        format!("{}\n", l.repo.display()),
    )
    .unwrap();
    let asks = l.run(&["land-lock", "push"]);
    assert!(
        !l.server.home.join("before-ran").exists(),
        "beforePush waits for the answer"
    );
    assert_eq!(code(&asks), 6, "{}", String::from_utf8_lossy(&asks.stderr));
    assert!(
        stdout(&asks).contains("ask: Add the feature"),
        "{}",
        stdout(&asks)
    );
    assert!(stdout(&asks).contains("why: the tab doesn't land without asking"));
    assert_eq!(l.on_origin(), before, "nothing pushed");
    assert_eq!(l.holder(), l.one.to_str().unwrap(), "the lock kept");

    l.server.ok(&["set", "-t", &l.tab, "@land", "1"]);
    l.server
        .ok(&["set", "-t", &l.tab, "@autopilot_item", "Add the feature"]);
    common::write_settings(
        &l.server.home,
        json!({ "tips": false, "github": false, "autopilot_landing_notices": true, "target_follows_default": false }),
    );
    let notices = relay_stand_in(&l.server.home);
    // Nothing but `--asked` follows it, so no rule naming it reaches that.
    let extra = l.run(&["land-lock", "push", "Add", "--asked"]);
    assert_eq!(code(&extra), 2, "{}", stdout(&extra));
    let landed = l.run(&["land-lock", "push"]);
    assert_eq!(
        code(&landed),
        0,
        "{}{}",
        stdout(&landed),
        String::from_utf8_lossy(&landed.stderr)
    );
    let head = rev(&l.one, "HEAD");
    assert!(l.server.home.join("before-ran").exists(), "beforePush ran");
    assert_eq!(l.on_origin(), head, "pushed");
    let said = stdout(&landed);
    assert!(said.starts_with("landed on main: "), "{said}");
    assert!(said.contains(&format!("before {clone_before}")), "{said}");
    assert_eq!(rev(&l.repo, "HEAD"), head, "the main clone fast-forwarded");
    assert_eq!(l.holder(), "", "the lock let go");
    let pushed = l.server.ok(&["show", "-t", &l.tab, "@autopilot_pushed"]);
    let pushed = pushed.trim();
    assert!(!pushed.is_empty() && head.starts_with(pushed), "{pushed}");
    assert_eq!(
        l.server
            .ok(&["show", "-t", &l.tab, "@autopilot_autoland"])
            .trim(),
        pushed,
        "said landed without asking"
    );
    assert!(
        !l.server
            .ok(&["show", "-t", &l.tab, "@tab_landed"])
            .trim()
            .is_empty(),
        "filed as landed"
    );
    assert_eq!(
        *notices.lock().unwrap(),
        [format!(
            "Landed {pushed} on main without asking: Add the feature"
        )],
        "the Slack relay sends it"
    );
    // Dismissed, then updated with what the change needs: no second notice.
    std::fs::write(l.server.state.join("autolanded.json"), "[]").unwrap();
    let updated = l.run(&[
        "land-lock",
        "autolanded",
        pushed,
        "--apply-range",
        "HEAD",
        "HEAD",
    ]);
    assert_eq!(
        code(&updated),
        0,
        "{}",
        String::from_utf8_lossy(&updated.stderr)
    );
    assert_eq!(notices.lock().unwrap().len(), 1, "sent once");
    // The landing kept with its item.
    let kept = || {
        let pending =
            std::fs::read_to_string(l.server.state.join("autolanded.json")).unwrap_or_default();
        let history = std::fs::read_dir(l.server.state.join("notifications"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|f| std::fs::read_to_string(f.path()).unwrap_or_default())
            .collect::<String>();
        pending.contains("Add the feature") || history.contains("Add the feature")
    };
    let deadline = common::deadline(Duration::from_secs(10));
    while !kept() {
        assert!(std::time::Instant::now() < deadline, "the landing kept");
        std::thread::sleep(Duration::from_millis(50));
    }

    let again = l.run(&["land-lock", "push"]);
    assert_eq!(code(&again), 3, "the lock no longer this worktree's");
}

#[test]
fn the_landing_push_asks_on_a_leak_or_a_changed_change_and_rebases_when_main_moved() {
    let l = Landing::new();
    l.server.ok(&["set", "-t", &l.tab, "@land", "1"]);
    let before = l.on_origin();
    let words = l.server.home.join(".config/switchboard/leak-check.txt");

    std::fs::write(&words, "feature\n").unwrap();
    let leak = l.run(&["land-lock", "push"]);
    assert_eq!(code(&leak), 6, "{}", stdout(&leak));
    assert!(stdout(&leak).contains("leak: "), "{}", stdout(&leak));
    std::fs::remove_file(&words).unwrap();

    // A fix after the review: no longer the change it found clean.
    std::fs::write(l.one.join("feature.txt"), "a feature, fixed\n").unwrap();
    git(&l.one, &["commit", "-q", "-am", "Fix the feature"]);
    let changed = l.run(&["land-lock", "push"]);
    assert_eq!(code(&changed), 6, "{}", stdout(&changed));
    assert!(stdout(&changed).contains("isn't the one the review found clean"));
    assert_eq!(l.on_origin(), before, "nothing pushed");

    // Another landing pushed meanwhile: rebase again, the lock kept.
    let other = l.server.home.join("other");
    git(
        &l.server.home,
        &[
            "clone",
            "-q",
            "-b",
            "main",
            l.origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    git(&other, &["commit", "-q", "--allow-empty", "-m", "late"]);
    git(&other, &["push", "-q", "origin", "HEAD:main"]);
    let moved = l.run(&["land-lock", "push", "--asked"]);
    assert_eq!(code(&moved), 5, "{}", stdout(&moved));
    assert_eq!(l.holder(), l.one.to_str().unwrap(), "the lock kept");
}

#[test]
fn a_refused_landing_push_leaves_the_remote_and_lets_the_lock_go() {
    use std::os::unix::fs::PermissionsExt;
    let l = Landing::new();
    let before = l.on_origin();
    let hooks = l.server.home.join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-push");
    std::fs::write(&hook, "#!/bin/sh\necho no review recorded >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(
        &l.repo,
        &["config", "core.hooksPath", hooks.to_str().unwrap()],
    );
    // Its beforePush, the repository's code, runs only in a worktree of
    // your own commits or a trusted clone; this one's commits name no one
    // the computer's git knows.
    std::fs::write(
        l.server.home.join(".config/switchboard/repos.json"),
        json!({ l.repo.join(".git").to_str().unwrap(): { "landing": "direct", "test": "true", "beforePush": "touch \"$HOME/before-ran\"" } })
            .to_string(),
    )
    .unwrap();
    let untrusted = l.run(&["land-lock", "push", "--asked"]);
    assert_eq!(code(&untrusted), 1);
    assert!(String::from_utf8_lossy(&untrusted.stderr).contains("trusted-dirs"));
    assert!(!l.server.home.join("before-ran").exists(), "nothing ran");
    assert_eq!(l.holder(), "", "the lock let go");
    std::fs::write(
        l.server.home.join(".config/switchboard/trusted-dirs"),
        format!("{}\n", l.repo.display()),
    )
    .unwrap();
    assert_eq!(code(&l.run(&["land-lock", "take"])), 0);

    let refused = l.run(&["land-lock", "push", "--asked"]);
    assert!(
        l.server.home.join("before-ran").exists(),
        "beforePush ran: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(code(&refused), 1, "{}", stdout(&refused));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("no review recorded"),
        "the hook's words said"
    );
    assert_eq!(l.on_origin(), before, "the remote unchanged");
    assert_eq!(l.holder(), "", "the lock let go");
}

#[test]
fn ask_before_landing_chosen_while_before_push_runs_asks_after_all() {
    let l = Landing::new();
    l.server.ok(&["set", "-t", &l.tab, "@land", "1"]);
    let before = l.on_origin();
    // beforePush stands in for you choosing "Ask before landing" meanwhile.
    std::fs::write(
        l.server.home.join(".config/switchboard/repos.json"),
        json!({ l.repo.join(".git").to_str().unwrap(): { "landing": "direct", "test": "true", "review": "review-loop", "beforePush": format!("env -u SWITCHBOARD_PANE '{BIN}' set -t {} @land 0", l.tab) } })
            .to_string(),
    )
    .unwrap();
    std::fs::write(
        l.server.home.join(".config/switchboard/trusted-dirs"),
        format!("{}\n", l.repo.display()),
    )
    .unwrap();
    let asks = l.run(&["land-lock", "push"]);
    assert_eq!(
        code(&asks),
        6,
        "{}{}",
        stdout(&asks),
        String::from_utf8_lossy(&asks.stderr)
    );
    assert_eq!(l.on_origin(), before, "nothing pushed");
    assert_eq!(l.holder(), l.one.to_str().unwrap(), "the lock kept");
}

/// `land-lock squash` run in `repo` with `args`.
fn squash(home: &Path, repo: &Path, args: &[&str]) -> Output {
    let mut command = land_lock(home, repo, &["squash"]);
    command
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e");
    command.output().unwrap()
}

fn commit_file(repo: &Path, name: &str) {
    std::fs::write(repo.join(name), format!("{name}\n")).unwrap();
    git(repo, &["add", name]);
    git(repo, &["commit", "-q", "-m", name]);
}

/// The clean review the review loop records, for HEAD on `base`.
fn record_review(home: &Path, repo: &Path, base: &str) {
    let patch = Command::new(BIN)
        .args(["codex-review", "--patch-id", base, "HEAD"])
        .current_dir(repo)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        patch.status.success(),
        "{}",
        String::from_utf8_lossy(&patch.stderr)
    );
    let record = repo
        .join(rev(repo, "--absolute-git-dir"))
        .join("review-loop-clean");
    std::fs::write(
        record,
        format!(
            "{} {} {}\n",
            rev(repo, "HEAD"),
            rev(repo, base),
            String::from_utf8_lossy(&patch.stdout).trim()
        ),
    )
    .unwrap();
}

#[test]
fn squash_folds_the_branch_into_one_commit_and_moves_the_review_only_when_safe() {
    let tmp = TempDir::new("land-squash");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let repo = repo(tmp.path());
    let base = rev(&repo, "HEAD");
    for name in ["a", "b", "c"] {
        commit_file(&repo, name);
    }
    let message = tmp.path().join("message.txt");
    std::fs::write(&message, "One feature\n\nWhat it does.\n").unwrap();
    let message = message.to_str().unwrap();

    // Commits not under your git email are someone else's, whose commit
    // hooks it won't run.
    let out = squash(&home, &repo, &["--message-file", message, "--onto", &base]);
    assert_eq!(code(&out), 1);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("isn't yours"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    git(&repo, &["config", "user.email", "t@e"]);

    // Something uncommitted refuses, changing nothing, untracked files
    // included even where status hides them.
    git(&repo, &["config", "status.showUntrackedFiles", "no"]);
    let head = rev(&repo, "HEAD");
    std::fs::write(repo.join("d"), "d\n").unwrap();
    let out = squash(&home, &repo, &["--message-file", message, "--onto", &base]);
    assert_eq!(code(&out), 1, "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(rev(&repo, "HEAD"), head);
    std::fs::remove_file(repo.join("d")).unwrap();

    // A commit after the reviewed one refuses.
    record_review(&home, &repo, &base);
    commit_file(&repo, "late");
    let out = squash(&home, &repo, &["--message-file", message, "--onto", &base]);
    assert_eq!(code(&out), 1);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("past the reviewed commit"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    git(&repo, &["reset", "-q", "--hard", "HEAD^"]);

    // Three commits become one of the same tree, the review moved to it.
    let tree = rev(&repo, "HEAD^{tree}");
    let out = squash(&home, &repo, &["--message-file", message, "--onto", &base]);
    assert_eq!(code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(rev(&repo, "HEAD^"), base);
    assert_eq!(rev(&repo, "HEAD^{tree}"), tree);
    let record = repo
        .join(rev(&repo, "--absolute-git-dir"))
        .join("review-loop-clean");
    let recorded = std::fs::read_to_string(&record).unwrap();
    assert!(recorded.starts_with(&rev(&repo, "HEAD")), "{recorded}");
    let backup = format!("backup/pre-squash-{}", &head[..7]);
    assert_eq!(rev(&repo, &backup), head, "the backup is the commit before");
    // The review moved, so the hooks below fold commits no review covers.
    std::fs::remove_file(&record).unwrap();

    // A commit hook that stages a change and fails has the backup restored.
    commit_file(&repo, "e");
    let before = rev(&repo, "HEAD");
    let hook = repo.join(".git/hooks/pre-commit");
    std::fs::write(
        &hook,
        "#!/bin/sh\necho x > a && git add a && echo hook says no && exit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let out = squash(&home, &repo, &["--message-file", message, "--onto", &base]);
    assert_eq!(code(&out), 1, "{}", String::from_utf8_lossy(&out.stdout));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("the commit failed")
            && String::from_utf8_lossy(&out.stderr).contains("hook says no"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(rev(&repo, "HEAD"), before);
    assert_eq!(std::fs::read_to_string(repo.join("a")).unwrap(), "a\n");
    let dirty = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(
        dirty.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&dirty.stdout)
    );

    // A commit hook that changes the tree has the backup restored.
    std::fs::write(&hook, "#!/bin/sh\necho x > extra && git add extra\n").unwrap();
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let out = squash(&home, &repo, &["--message-file", message, "--onto", &base]);
    assert_eq!(code(&out), 1, "{}", String::from_utf8_lossy(&out.stdout));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("tree differs"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(rev(&repo, "HEAD"), before);
    assert!(!repo.join("extra").exists());
    std::fs::remove_file(&hook).unwrap();

    // Once pushed, the backups go.
    let out = squash(&home, &repo, &["--drop-backup"]);
    assert_eq!(code(&out), 0);
    let left = Command::new("git")
        .args(["branch", "--list", "backup/*"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(
        left.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&left.stdout)
    );
}

#[test]
fn queue_pr_keeps_order_across_worktrees_and_refuses_a_pr_twice() {
    let tmp = TempDir::new("land-lock-prs");
    let home = tmp.path();
    common::write_settings(home, json!({ "pr_merge_queue": true }));
    let repo = repo(home);
    let one = worktree(home, &repo, home, "one");
    let two = worktree(home, &repo, home, "two");

    let first = lock(home, &one, &["queue-pr", "12"]);
    assert_eq!(
        (code(&first), stdout(&first)),
        (0, "queued 1 of 1\n".into())
    );
    let second = lock(home, &two, &["queue-pr", "#7"]);
    assert_eq!(
        (code(&second), stdout(&second)),
        (0, "queued 2 of 2\n".into())
    );
    let again = lock(home, &two, &["queue-pr", "12"]);
    assert_eq!(code(&again), 3, "a PR is queued once");
    assert!(
        stdout(&again).contains("queued 1 of 2"),
        "{}",
        stdout(&again)
    );

    let queue = stdout(&lock(home, &repo, &["pr-queue"]));
    let numbers: Vec<&str> = queue
        .lines()
        .map(|l| l.split('\t').nth(1).unwrap())
        .collect();
    assert_eq!(numbers, ["#12", "#7"], "shared by every worktree, in order");
    assert!(queue.contains(&one.display().to_string()));

    assert_eq!(code(&lock(home, &one, &["move-pr", "7", "first"])), 0);
    assert_eq!(
        stdout(&lock(home, &one, &["pr-status", "7"])),
        "queued 1 of 2\n"
    );
    common::write_settings(home, json!({}));
    assert_eq!(
        stdout(&lock(home, &one, &["pr-status", "7"])),
        "held Settings' pr_merge_queue is off · queued 1 of 2\n",
        "with the setting off, nothing moves a queued PR"
    );
    common::write_settings(home, json!({ "pr_merge_queue": true }));
    assert_eq!(code(&lock(home, &one, &["dequeue-pr", "7"])), 0);
    assert_eq!(code(&lock(home, &one, &["dequeue-pr", "7"])), 3);
    assert_eq!(stdout(&lock(home, &one, &["pr-status", "7"])), "none\n");
    assert_eq!(
        code(&lock(home, &one, &["queue-pr"])),
        2,
        "a number is needed"
    );
}
