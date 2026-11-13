//! `switchboard update-target`: a throwaway origin (a bare repository, which
//! the clone reaches through an `insteadOf` for a GitHub URL, so repos.json
//! names it) with a `ux` branch behind main, and a clone of it, in a
//! temporary home, so nothing touches your own repositories or settings.

use crate::common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use common::{BIN, TempDir};
use serde_json::json;

fn env(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e")
        .env("LAND_LOCK_POLL", "0.1")
        .env_remove("SWITCHBOARD")
        .env_remove("SWITCHBOARD_PANE")
        .env_remove("SWITCHBOARD_STATE_DIR")
        .env_remove("SWITCHBOARD_SETTINGS")
        .env_remove("CLAUDE_CODE_SESSION_ID");
}

fn git(home: &Path, dir: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args).stdin(Stdio::null());
    env(&mut cmd, home);
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn command(home: &Path, dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.arg("update-target")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    env(&mut cmd, home);
    cmd
}

fn run(home: &Path, dir: &Path, args: &[&str]) -> Output {
    command(home, dir, args).output().unwrap()
}

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The test's repositories: origin, the main clone, and a second clone that
/// moves origin's branches on as someone else would.
struct Repos {
    tmp: TempDir,
    origin: PathBuf,
    clone: PathBuf,
    other: PathBuf,
}

impl Repos {
    fn home(&self) -> &Path {
        self.tmp.path()
    }

    /// A commit on `branch` from the other clone, pushed, writing `file`.
    fn push_from_other(&self, branch: &str, file: &str, text: &str) {
        let (home, other) = (self.home(), &self.other);
        git(home, other, &["fetch", "-q", "origin"]);
        git(
            home,
            other,
            &["checkout", "-q", "-B", branch, &format!("origin/{branch}")],
        );
        fs::write(other.join(file), text).unwrap();
        git(home, other, &["add", file]);
        git(
            home,
            other,
            &["commit", "-q", "-m", &format!("{branch}: {file}")],
        );
        git(home, other, &["push", "-q", "origin", branch]);
    }

    fn origin_has(&self, branch: &str) -> String {
        git(self.home(), &self.origin, &["rev-parse", branch])
    }
}

fn repos(name: &str, test: Option<&str>) -> Repos {
    let tmp = TempDir::new(name);
    let home = tmp.path().to_path_buf();
    let s = |p: &Path| p.to_str().unwrap().to_string();
    let origin = home.join("origin.git");
    let clone = home.join("clone");
    let other = home.join("other");
    git(&home, &home, &["init", "-q", "--bare", &s(&origin)]);
    git(&home, &origin, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&home, &home, &["clone", "-q", &s(&origin), &s(&clone)]);
    git(&home, &clone, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    // Your git email, which the commits are written under, so the merge
    // holds only your own and origin's (src/trust.rs).
    git(&home, &clone, &["config", "user.email", "t@e"]);
    // origin by a GitHub URL, which repos.json names, reached on disk.
    git(
        &home,
        &clone,
        &["remote", "set-url", "origin", "git@github.com:acme/r.git"],
    );
    git(
        &home,
        &clone,
        &[
            "config",
            &format!("url.{}.insteadOf", s(&origin)),
            "git@github.com:acme/r.git",
        ],
    );
    fs::write(clone.join("shared.txt"), "one\n").unwrap();
    git(&home, &clone, &["add", "shared.txt"]);
    git(&home, &clone, &["commit", "-q", "-m", "start"]);
    git(&home, &clone, &["push", "-q", "origin", "main", "main:ux"]);
    git(&home, &clone, &["remote", "set-head", "origin", "main"]);
    git(&home, &clone, &["branch", "-q", "ux", "origin/ux"]);
    git(&home, &home, &["clone", "-q", &s(&origin), &s(&other)]);
    let own = home.join(".config/switchboard");
    fs::create_dir_all(&own).unwrap();
    let mut entry = json!({ "landing": "direct" });
    if let Some(test) = test {
        entry["test"] = json!(test);
    }
    fs::write(
        own.join("repos.json"),
        json!({ "acme/r": entry }).to_string(),
    )
    .unwrap();
    Repos {
        tmp,
        origin,
        clone,
        other,
    }
}

fn worktree_of(r: &Repos) -> PathBuf {
    r.clone.join(".claude/worktrees/update-ux")
}

#[test]
fn its_steps_wait_for_a_merge_holding_someone_else_s_commit_to_be_trusted() {
    let r = repos(
        "update-target-trust",
        Some("test -f fixed && echo ran >> \"$HOME/tests-ran\""),
    );
    let home = r.home();
    r.push_from_other("main", "a.txt", "from main\n");
    let ux_before = r.origin_has("ux");
    let out = run(home, &r.clone, &["ux"]);
    assert_eq!(out.status.code(), Some(3), "{}", said(&out));

    // A fix committed there under another email, on no remote.
    let at = worktree_of(&r);
    fs::write(at.join("fixed"), "").unwrap();
    git(home, &at, &["add", "fixed"]);
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(&at).args(["commit", "-q", "-m", "fix"]);
    env(&mut cmd, home);
    cmd.env("GIT_AUTHOR_EMAIL", "someone@else");
    assert!(cmd.status().unwrap().success());

    let out = run(home, &r.clone, &["ux"]);
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert!(said(&out).contains("trusted-dirs"), "{}", said(&out));
    assert!(
        !home.join("tests-ran").exists(),
        "nothing of the repository ran"
    );
    assert_eq!(r.origin_has("ux"), ux_before, "nothing pushed");

    fs::write(
        home.join(".config/switchboard/trusted-dirs"),
        format!("{}\n", r.clone.canonicalize().unwrap().display()),
    )
    .unwrap();
    let out = run(home, &r.clone, &["ux"]);
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
    assert!(home.join("tests-ran").exists());
}

#[test]
fn main_is_merged_into_the_target_tested_and_pushed_and_its_worktree_removed() {
    let r = repos("update-target", Some("echo ran >> \"$HOME/tests-ran\""));
    let home = r.home();
    r.push_from_other("main", "a.txt", "from main\n");
    r.push_from_other("ux", "b.txt", "from ux\n");
    let ux_before = r.origin_has("ux");

    let out = run(home, &r.clone, &["ux"]);
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
    assert!(said(&out).contains("merged main into ux"), "{}", said(&out));
    let ux = r.origin_has("ux");
    assert_eq!(
        git(home, &r.origin, &["rev-parse", &format!("{ux}^1")]),
        ux_before,
        "a merge on top of origin's ux, never forced"
    );
    assert_eq!(
        git(home, &r.origin, &["rev-parse", &format!("{ux}^2")]),
        r.origin_has("main")
    );
    assert_eq!(
        git(home, &r.origin, &["log", "-1", "--format=%s", "ux"]),
        "Merge main into ux"
    );
    assert_eq!(fs::read_to_string(home.join("tests-ran")).unwrap(), "ran\n");
    assert_eq!(
        git(home, &r.clone, &["rev-parse", "ux"]),
        ux,
        "the local ux followed"
    );
    assert!(!worktree_of(&r).exists(), "its worktree removed");
    let common = r.clone.join(".git");
    assert!(!common.join("land.lock.ux").exists(), "the lock let go");

    let again = run(home, &r.clone, &["ux"]);
    assert_eq!(again.status.code(), Some(0), "{}", said(&again));
    assert!(
        said(&again).contains("has main's work already"),
        "{}",
        said(&again)
    );
    assert_eq!(r.origin_has("ux"), ux, "nothing pushed");
    assert_eq!(fs::read_to_string(home.join("tests-ran")).unwrap(), "ran\n");
}

#[test]
fn a_conflict_is_left_in_its_worktree_and_a_second_run_finishes_it() {
    let r = repos("update-target-conflict", None);
    let home = r.home();
    r.push_from_other("main", "shared.txt", "main's\n");
    r.push_from_other("ux", "shared.txt", "ux's\n");
    let ux_before = r.origin_has("ux");

    let out = run(home, &r.clone, &["ux"]);
    assert_eq!(out.status.code(), Some(3), "{}", said(&out));
    assert!(said(&out).contains("shared.txt"), "{}", said(&out));
    assert!(
        said(&out).contains("switchboard update-target ux"),
        "{}",
        said(&out)
    );
    assert_eq!(r.origin_has("ux"), ux_before, "nothing pushed");
    let at = worktree_of(&r);
    assert!(at.join("shared.txt").exists());
    git(home, &at, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]);
    assert!(
        !r.clone.join(".git/land.lock.ux").exists(),
        "the lock let go meanwhile"
    );

    let unresolved = run(home, &r.clone, &["ux"]);
    assert_eq!(unresolved.status.code(), Some(3), "{}", said(&unresolved));

    fs::write(at.join("shared.txt"), "both\n").unwrap();
    git(home, &at, &["add", "shared.txt"]);
    // A change to a file not added stops it before the merge is committed.
    fs::write(at.join("shared.txt"), "both, and more\n").unwrap();
    let unadded = run(home, &r.clone, &["ux"]);
    assert_eq!(unadded.status.code(), Some(3), "{}", said(&unadded));
    assert!(said(&unadded).contains("not added"), "{}", said(&unadded));
    git(home, &at, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]);
    fs::write(at.join("shared.txt"), "both\n").unwrap();
    let out = run(home, &r.clone, &["ux"]);
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
    assert_eq!(
        git(home, &r.origin, &["show", "ux:shared.txt"]),
        "both",
        "the resolution pushed"
    );
    assert!(!at.exists());
}

#[test]
fn failing_tests_push_nothing_and_a_fix_committed_there_lands() {
    let r = repos("update-target-tests", Some("test -f fixed"));
    let home = r.home();
    r.push_from_other("main", "a.txt", "from main\n");
    let ux_before = r.origin_has("ux");

    let out = run(home, &r.clone, &["ux"]);
    assert_eq!(out.status.code(), Some(3), "{}", said(&out));
    assert!(said(&out).contains("the tests failed"), "{}", said(&out));
    assert_eq!(r.origin_has("ux"), ux_before, "nothing pushed");

    let at = worktree_of(&r);
    // A fix left as a new file, not committed, is never tested without
    // being pushed, nor thrown away.
    fs::write(at.join("fixed"), "").unwrap();
    let out = run(home, &r.clone, &["ux"]);
    assert_eq!(out.status.code(), Some(3), "{}", said(&out));
    assert!(said(&out).contains("not committed"), "{}", said(&out));
    assert!(at.join("fixed").exists());
    assert_eq!(r.origin_has("ux"), ux_before, "nothing pushed");

    git(home, &at, &["add", "fixed"]);
    git(home, &at, &["commit", "-q", "-m", "fix"]);
    let out = run(home, &r.clone, &["ux"]);
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
    assert_eq!(
        git(home, &r.origin, &["log", "-1", "--format=%s", "ux"]),
        "fix",
        "the merge and its fix pushed"
    );
}

#[test]
fn the_default_branch_and_a_branch_origin_lacks_are_refused() {
    let r = repos("update-target-refused", None);
    let home = r.home();
    let out = run(home, &r.clone, &["main"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        said(&out).contains("is the default branch"),
        "{}",
        said(&out)
    );
    let out = run(home, &r.clone, &["nowhere"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        said(&out).contains("nowhere isn't on origin"),
        "{}",
        said(&out)
    );
    assert!(!r.clone.join(".claude/worktrees/update-nowhere").exists());

    // A directory of someone's at the worktree's path is left alone.
    let theirs = worktree_of(&r);
    fs::create_dir_all(&theirs).unwrap();
    let out = run(home, &r.clone, &["ux"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        said(&out).contains("isn't a detached worktree"),
        "{}",
        said(&out)
    );
    assert_eq!(git(home, &r.clone, &["branch", "--show-current"]), "main");
}

#[test]
fn a_target_origin_already_brought_up_to_date_moves_the_local_branch_too() {
    let r = repos("update-target-local", None);
    let home = r.home();
    r.push_from_other("main", "a.txt", "from main\n");
    // Another clone merged main into ux already.
    git(home, &r.other, &["checkout", "-q", "-B", "ux", "origin/ux"]);
    git(home, &r.other, &["merge", "-q", "--no-edit", "main"]);
    git(home, &r.other, &["push", "-q", "origin", "ux"]);

    let out = run(home, &r.clone, &["ux"]);
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
    assert!(
        said(&out).contains("has main's work already"),
        "{}",
        said(&out)
    );
    assert_eq!(
        git(home, &r.clone, &["rev-parse", "ux"]),
        r.origin_has("ux")
    );
}

#[test]
fn it_waits_while_a_landing_onto_the_target_holds_its_lock() {
    let r = repos("update-target-wait", None);
    let home = r.home();
    r.push_from_other("main", "a.txt", "from main\n");
    // A landing onto ux, in a worktree with a running Claude session (this
    // test's own process stands in for it).
    let lander = home.join("lander");
    git(
        home,
        &r.clone,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "item",
            lander.to_str().unwrap(),
            "ux",
        ],
    );
    let sessions = home.join(".claude/sessions");
    fs::create_dir_all(&sessions).unwrap();
    fs::write(
        sessions.join("lander.json"),
        json!({ "sessionId": "lander", "cwd": lander, "pid": std::process::id() }).to_string(),
    )
    .unwrap();
    let lock = |args: &[&str]| {
        let mut cmd = Command::new(BIN);
        cmd.arg("land-lock")
            .args(["--target", "ux"])
            .args(args)
            .current_dir(&lander)
            .stdin(Stdio::null());
        env(&mut cmd, home);
        cmd.env("CLAUDE_CODE_SESSION_ID", "lander");
        cmd.output().unwrap()
    };
    assert_eq!(lock(&["take"]).status.code(), Some(0));

    let mut waiting: Child = command(home, &r.clone, &["ux"]).spawn().unwrap();
    let queued = common::deadline(Duration::from_secs(10));
    loop {
        let queue = fs::read_to_string(r.clone.join(".git/land.queue.ux")).unwrap_or_default();
        if queue.contains("update-ux") {
            break;
        }
        assert!(Instant::now() < queued, "it never joined the queue");
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(500) * common::patience());
    assert!(
        waiting.try_wait().unwrap().is_none(),
        "it waits for the lock"
    );
    // A second run for the same branch meanwhile stops rather than share
    // its worktree.
    let second = run(home, &r.clone, &["ux"]);
    assert_eq!(second.status.code(), Some(1), "{}", said(&second));
    assert!(said(&second).contains("is under way"), "{}", said(&second));
    assert!(
        waiting.try_wait().unwrap().is_none(),
        "the first still waits"
    );
    assert!(lock(&["release"]).status.success());

    let done = common::deadline(Duration::from_secs(30));
    let status = loop {
        if let Some(status) = waiting.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < done, "it never took the lock");
        std::thread::sleep(Duration::from_millis(50));
    };
    let out = waiting.wait_with_output().unwrap();
    assert_eq!(status.code(), Some(0), "{}", said(&out));
    assert_eq!(
        r.origin_has("ux"),
        r.origin_has("main"),
        "ux, with nothing of its own, fast-forwarded to main"
    );
}

#[test]
fn a_landing_on_main_while_its_tests_run_is_merged_in_before_the_push() {
    // The first run of the tests moves origin's main on, as a landing would.
    let r = repos(
        "update-target-moved",
        Some(
            "echo ran >> \"$HOME/tests-ran\"; if [ ! -f \"$HOME/once\" ]; then touch \"$HOME/once\" && git -C \"$HOME/other\" commit -q --allow-empty -m late && git -C \"$HOME/other\" push -q origin HEAD:main; fi",
        ),
    );
    let home = r.home();
    r.push_from_other("ux", "b.txt", "from ux\n");
    r.push_from_other("main", "a.txt", "from main\n");

    let out = run(home, &r.clone, &["ux"]);
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
    let late = r.origin_has("main");
    assert_eq!(
        git(home, &r.origin, &["log", "-1", "--format=%s", &late]),
        "late"
    );
    git(
        home,
        &r.origin,
        &["merge-base", "--is-ancestor", &late, "ux"],
    );
    assert_eq!(
        fs::read_to_string(home.join("tests-ran")).unwrap(),
        "ran\nran\n",
        "tested again with it"
    );
}

#[test]
fn a_target_tracking_another_remote_branch_is_updated_there() {
    let r = repos("update-target-tracked", None);
    let home = r.home();
    // The local qa lands on origin's ux, as a landing onto it pushes there.
    git(
        home,
        &r.clone,
        &["branch", "-q", "--track", "qa", "origin/ux"],
    );
    r.push_from_other("main", "a.txt", "from main\n");

    let out = run(home, &r.clone, &["qa"]);
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
    assert_eq!(
        r.origin_has("ux"),
        r.origin_has("main"),
        "origin's ux updated"
    );
    assert_eq!(
        git(
            home,
            &r.origin,
            &["for-each-ref", "--format=%(refname)", "refs/heads/qa"]
        ),
        "",
        "no qa made on origin"
    );
    assert_eq!(
        git(home, &r.clone, &["rev-parse", "qa"]),
        r.origin_has("ux")
    );
}

#[test]
fn a_repos_json_that_cant_be_read_stops_it_before_anything_is_pushed() {
    let r = repos("update-target-broken", None);
    let home = r.home();
    r.push_from_other("main", "a.txt", "from main\n");
    let ux_before = r.origin_has("ux");
    fs::write(home.join(".config/switchboard/repos.json"), "{ broken").unwrap();
    let out = run(home, &r.clone, &["ux"]);
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert!(said(&out).contains("can't be read"), "{}", said(&out));
    assert_eq!(r.origin_has("ux"), ux_before, "nothing pushed");
}

#[test]
fn a_repository_off_github_takes_its_steps_by_its_git_directory() {
    let r = repos("update-target-off-github", None);
    let home = r.home();
    // origin reached by its path alone, as a repository off GitHub is.
    git(
        home,
        &r.clone,
        &["remote", "set-url", "origin", r.origin.to_str().unwrap()],
    );
    let common = r.clone.join(".git").canonicalize().unwrap();
    fs::write(
        home.join(".config/switchboard/repos.json"),
        json!({ common.to_str().unwrap(): { "landing": "direct", "test": "false" } }).to_string(),
    )
    .unwrap();
    r.push_from_other("main", "a.txt", "from main\n");
    let out = run(home, &r.clone, &["ux"]);
    assert_eq!(
        out.status.code(),
        Some(3),
        "its test ran and failed: {}",
        said(&out)
    );
}

#[test]
fn steps_that_change_a_tracked_file_leave_it_rather_than_push_something_else() {
    let r = repos("update-target-changed", Some("echo changed >> shared.txt"));
    let home = r.home();
    r.push_from_other("main", "a.txt", "from main\n");
    let ux_before = r.origin_has("ux");
    let out = run(home, &r.clone, &["ux"]);
    assert_eq!(out.status.code(), Some(3), "{}", said(&out));
    assert!(
        said(&out).contains("changed files git tracks"),
        "{}",
        said(&out)
    );
    assert_eq!(r.origin_has("ux"), ux_before, "nothing pushed");
    assert!(worktree_of(&r).exists());
}

#[test]
fn a_run_cut_short_while_holding_the_lock_is_taken_up_by_the_next_as_its_own() {
    let r = repos("update-target-cut-short", None);
    let home = r.home();
    r.push_from_other("main", "a.txt", "from main\n");
    // A run that made its worktree and took the lock, then ended without
    // letting go (its tab closed): its holder names a process that is gone.
    let at = worktree_of(&r);
    git(
        home,
        &r.clone,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            at.to_str().unwrap(),
            "origin/ux",
        ],
    );
    let at = at.canonicalize().unwrap();
    let lock = r.clone.join(".git/land.lock.ux");
    fs::create_dir_all(&lock).unwrap();
    let gone = 999_999;
    fs::write(
        lock.join("holder"),
        json!({ "path": at, "since": 1, "pid": gone }).to_string(),
    )
    .unwrap();

    // The next run is held at its tests, so the holder can be read.
    let own = home.join(".config/switchboard");
    fs::write(
        own.join("repos.json"),
        json!({ "acme/r": { "landing": "direct", "test": "while [ ! -f \"$HOME/go\" ]; do sleep 0.05; done" } })
            .to_string(),
    )
    .unwrap();
    let mut next = command(home, &r.clone, &["ux"]).spawn().unwrap();
    let deadline = common::deadline(Duration::from_secs(20));
    let holder = loop {
        let holder: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(lock.join("holder")).unwrap_or_default())
                .unwrap_or_default();
        if holder["pid"].as_i64().is_some_and(|p| p != gone) {
            break holder;
        }
        assert!(
            Instant::now() < deadline,
            "the holder still names the gone run: {holder}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(holder["pid"].as_i64(), Some(i64::from(next.id())));
    fs::write(home.join("go"), "").unwrap();
    let status = next.wait().unwrap();
    assert!(status.success());
}

#[test]
fn a_report_the_tests_leave_goes_with_the_worktree() {
    let r = repos("update-target-report", Some("echo ok > test-results.xml"));
    let home = r.home();
    r.push_from_other("main", "a.txt", "from main\n");
    let out = run(home, &r.clone, &["ux"]);
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
    assert!(!worktree_of(&r).exists(), "{}", said(&out));
}
