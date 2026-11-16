//! queue-and-clean's `switchboard land-lock test`, in a worktree of a
//! temporary repository with a temporary HOME and repos.json: a stand-in
//! test command passing and failing, recorded in the landing's record with
//! the counts it printed, no command set or an empty one; and, with the
//! tests in CI, the landing's `ci/land-<id>` branch pushed to a bare
//! repository standing in for GitHub's (an `insteadOf` from its github.com
//! address) and deleted after a pass, a failure and an interrupt, against a
//! small GitHub of the test's own on 127.0.0.1, whose default branch's
//! run tells the change's failure from its own.

use crate::common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use common::{BIN, TempDir};
use serde_json::{Value, json};

pub(crate) fn git(cwd: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository on `main` with a commit, and a worktree of it on `feature`
/// with one more, in `tmp`; the worktree's path.
pub(crate) fn worktree(tmp: &Path) -> PathBuf {
    let repo = tmp.join("repo");
    git(tmp, &["init", "-q", repo.to_str().unwrap()]);
    git(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    let dir = tmp.join("feature");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            dir.to_str().unwrap(),
        ],
    );
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-q", "-m", "a change"]);
    dir
}

/// repos.json in `home` with `entry` under `key`.
pub(crate) fn repos(home: &Path, key: &str, entry: Value) {
    let dir = home.join(".config/switchboard");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("repos.json"), json!({ key: entry }).to_string()).unwrap();
}

/// Lists `dir` in trusted-dirs in `home`, as you would to land a clone
/// with no remote of yours.
pub(crate) fn trust(home: &Path, dir: &Path) {
    let config = home.join(".config/switchboard");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("trusted-dirs"), format!("{}\n", dir.display())).unwrap();
}

fn common_dir(dir: &Path) -> String {
    let common = git(dir, &["rev-parse", "--git-common-dir"]);
    dir.join(common)
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn land_test(home: &Path, cwd: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(BIN);
    command
        .arg("land-lock")
        .arg("test")
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("SWITCHBOARD")
        .env_remove("SWITCHBOARD_PANE")
        .env_remove("SWITCHBOARD_SETTINGS")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .stdin(Stdio::null());
    command
}

pub(crate) fn said(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub(crate) fn record(dir: &Path) -> Value {
    let own = PathBuf::from(git(dir, &["rev-parse", "--absolute-git-dir"]));
    serde_json::from_str(&std::fs::read_to_string(own.join("landing.json")).unwrap()).unwrap()
}

#[test]
fn tests_run_here_are_recorded_with_their_counts_passing_or_failing() {
    let tmp = TempDir::new("land-test-here");
    let home = tmp.path();
    let dir = worktree(home);
    let key = common_dir(&dir);
    let run = |test: Value, args: &[&str]| {
        repos(home, &key, json!({ "landing": "direct", "test": test }));
        land_test(home, &dir, args).output().unwrap()
    };

    let untrusted = run(json!("touch ran"), &[]);
    assert_eq!(
        (untrusted.status.code(), said(&untrusted).lines().last()),
        (Some(7), Some("untrusted")),
        "a worktree whose commits lead back to no remote of yours runs nothing"
    );
    assert!(!dir.join("ran").exists());
    trust(home, &home.join("repo"));

    let passing = run(
        json!("echo 'test result: ok. 3 passed; 0 failed; 0 ignored'"),
        &[],
    );
    assert_eq!(passing.status.code(), Some(0), "{}", said(&passing));
    assert_eq!(said(&passing), "passed 3 of 3\n");
    let r = record(&dir);
    assert_eq!(
        (
            &r["tests"]["ok"],
            &r["tests"]["passed"],
            &r["tests"]["total"]
        ),
        (&json!(true), &json!(3), &json!(3))
    );
    assert_eq!(r["output"]["of"], "tests");

    let failing = run(
        json!(
            "echo '---- tabs::names stdout ----'; echo 'test result: FAILED. 2 passed; 1 failed; 0 ignored'; exit 101"
        ),
        &[],
    );
    assert_eq!(failing.status.code(), Some(1));
    let last = said(&failing).lines().last().unwrap().to_string();
    assert!(last.starts_with("failed 1 of 3 "), "{last}");
    assert!(last.ends_with("landing-test.log"), "{last}");
    assert!(said(&failing).contains("---- tabs::names stdout ----"));
    let r = record(&dir);
    assert_eq!(
        (
            &r["tests"]["ok"],
            &r["tests"]["failed"],
            &r["tests"]["file"]
        ),
        (&json!(false), &json!(1), &json!("tabs::names"))
    );
    assert_eq!(
        r["output"]["tail"][1],
        "test result: FAILED. 2 passed; 1 failed; 0 ignored"
    );

    let unset = run(Value::Null, &[]);
    assert_eq!(unset.status.code(), Some(5));
    assert!(said(&unset).ends_with("not set\n"));
    let given = run(Value::Null, &["--command", "true"]);
    assert_eq!(
        (given.status.code(), said(&given)),
        (Some(0), "passed\n".to_string()),
        "a command given runs in its place"
    );
    let late = run(Value::Null, &["--base", "main", "--command", "true"]);
    assert_eq!(
        late.status.code(),
        Some(2),
        "--command after the other arguments is refused, so a rule for --base * never covers it"
    );
    let none = run(json!(""), &[]);
    assert_eq!(
        (none.status.code(), said(&none)),
        (Some(0), "no tests\n".to_string())
    );

    // A target that tracks no remote has nowhere to push: the tests run
    // here, with no published base to check.
    git(&dir, &["branch", "dev", "main"]);
    git(&dir, &["config", "branch.feature.wtnTarget", "dev"]);
    repos(
        home,
        &key,
        json!({ "landing": "direct", "test": "echo local", "testIn": "ci" }),
    );
    let local = land_test(home, &dir, &["--base", "dev"]).output().unwrap();
    assert_eq!(
        (local.status.code(), said(&local).lines().last()),
        (Some(0), Some("passed")),
        "{}",
        said(&local)
    );
}

#[cfg(feature = "net")]
mod in_ci {
    use super::*;
    use common::http::{self, Reply};
    use common::stand_in;
    use std::time::Duration;

    /// The worktree's repository with `origin` at github.com/o/r, which an
    /// `insteadOf` sends to a bare repository in `tmp`, `main` pushed there
    /// and the main clone trusted; the bare one.
    fn origin(tmp: &Path, dir: &Path) -> PathBuf {
        let bare = tmp.join("origin.git");
        git(tmp, &["init", "-q", "--bare", bare.to_str().unwrap()]);
        let url = "https://github.com/o/r.git";
        git(dir, &["remote", "add", "origin", url]);
        git(
            dir,
            &["config", &format!("url.{}.insteadOf", bare.display()), url],
        );
        git(dir, &["push", "-q", "origin", "main"]);
        git(dir, &["fetch", "-q", "origin"]);
        trust(tmp, &tmp.join("repo"));
        bare
    }

    /// The bare repository's `ci/land-*` branches.
    fn ci_branches(bare: &Path) -> String {
        git(
            bare,
            &["for-each-ref", "--format=%(refname)", "refs/heads/ci/"],
        )
    }

    /// A GitHub whose check runs are `base` for the commit `base_sha` and
    /// `runs` for every other, every commit with no statuses, and job 5's
    /// log failing `tabs::names` under nextest.
    fn github(runs: Value, base_sha: String, base: Value) -> http::Fake {
        http::Fake::github("test-token", move |request| {
            if request.method != "GET" {
                return None;
            }
            if request.target == "/repos/o/r/actions/jobs/5/logs" {
                let log =
                    "2026-11-15T10:00:00.1Z         FAIL [   1.204s] switchboard::it tabs::names";
                return Some(Reply::json(200, Value::String(log.into())));
            }
            if !request.target.starts_with("/repos/o/r/commits/") {
                return None;
            }
            let body = if request.target.contains("/check-runs") {
                let runs = if request.target.contains(&format!("/commits/{base_sha}/")) {
                    &base
                } else {
                    &runs
                };
                json!({ "total_count": runs.as_array().map_or(0, Vec::len), "check_runs": runs })
            } else {
                json!({ "state": "pending", "statuses": [] })
            };
            Some(Reply::json(200, body))
        })
    }

    fn in_ci(home: &Path, dir: &Path, fake: &http::Fake) -> Command {
        in_ci_from(home, dir, fake, "main")
    }

    fn in_ci_from(home: &Path, dir: &Path, fake: &http::Fake, base: &str) -> Command {
        let mut command = land_test(home, dir, &["--base", base]);
        command
            .env("PATH", stand_in::path_with_gh(&home.join("bin")))
            .env("GH_TOKEN", "test-token")
            .env("SWITCHBOARD_GITHUB_API", &fake.url)
            .env("WAIT_CI_POLL_SECS", "0.05")
            .env_remove("GITHUB_TOKEN");
        command
    }

    fn check(conclusion: Value, status: &str) -> Value {
        json!({ "name": "Tests", "status": status, "conclusion": conclusion,
            "details_url": "https://github.com/o/r/actions/runs/77/job/5" })
    }

    #[test]
    fn the_landing_s_branch_is_pushed_and_deleted_after_a_pass_and_a_failure() {
        let tmp = TempDir::new("land-test-ci");
        let home = tmp.path();
        let dir = worktree(home);
        let bare = origin(home, &dir);
        repos(
            home,
            "o/r",
            json!({ "landing": "direct", "test": "false", "testIn": "ci" }),
        );

        let head = git(&dir, &["rev-parse", "HEAD"]);
        let fake = github(json!([]), String::new(), json!([]));
        let unpublished = in_ci_from(home, &dir, &fake, &head).output().unwrap();
        assert_eq!(
            unpublished.status.code(),
            Some(2),
            "a base that isn't on a remote would leave the change out of leak-check: {}",
            said(&unpublished)
        );
        assert_eq!(ci_branches(&bare), "", "nothing pushed");

        let main = git(&dir, &["rev-parse", "main"]);
        let passing = json!([check(json!("success"), "completed")]);
        let fake = github(passing.clone(), main.clone(), passing.clone());
        let passed = in_ci(home, &dir, &fake).output().unwrap();
        assert_eq!(passed.status.code(), Some(0), "{}", said(&passed));
        assert_eq!(said(&passed), "passed\n");
        assert_eq!(record(&dir)["tests"]["ok"], true);
        assert_eq!(record(&dir)["leak"]["found"], 0);
        assert_eq!(ci_branches(&bare), "", "deleted after a pass");
        assert!(
            git(&bare, &["cat-file", "-t", &head]) == "commit",
            "the landing's commit was pushed"
        );

        let failing = json!([check(json!("failure"), "completed")]);
        let fake = github(failing.clone(), main.clone(), passing);
        let failed = in_ci(home, &dir, &fake).output().unwrap();
        assert_eq!(failed.status.code(), Some(1));
        assert_eq!(
            said(&failed),
            format!("failed in CI {head} 77 Tests\n"),
            "a test that passed on the default branch is the change's"
        );
        assert_eq!(record(&dir)["tests"]["file"], "Tests");
        assert_eq!(record(&dir)["output"]["of"], "tests");
        let own = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
        let log = std::fs::read_to_string(own.join("landing-test.log")).unwrap();
        assert!(
            log.contains("==> Tests <==") && log.contains("tabs::names"),
            "the failed job's log is kept as the output: {log}"
        );
        assert_eq!(ci_branches(&bare), "", "deleted after a failure");

        let fake = github(failing.clone(), main, failing);
        let base_failing = in_ci(home, &dir, &fake).output().unwrap();
        assert_eq!(base_failing.status.code(), Some(4));
        assert_eq!(
            said(&base_failing),
            format!("base-failing in CI {head} 77 Tests\n"),
            "the same test failing on the default branch is its own"
        );
        assert_eq!(
            ci_branches(&bare),
            "",
            "deleted after the default branch's failure"
        );
    }

    #[test]
    fn the_landing_s_branch_is_deleted_when_the_wait_is_interrupted() {
        let tmp = TempDir::new("land-test-int");
        let home = tmp.path();
        let dir = worktree(home);
        let bare = origin(home, &dir);
        repos(home, "o/r", json!({ "landing": "direct", "testIn": "ci" }));
        let fake = github(
            json!([check(Value::Null, "in_progress")]),
            git(&dir, &["rev-parse", "main"]),
            json!([]),
        );
        let mut child = in_ci(home, &dir, &fake)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = common::deadline(Duration::from_secs(10));
        while ci_branches(&bare).is_empty() {
            assert!(std::time::Instant::now() < deadline, "never pushed");
            std::thread::sleep(Duration::from_millis(50));
        }
        // The push is in; give the handlers' thread the moment it takes to
        // start, scaled as every wait is.
        std::thread::sleep(Duration::from_millis(200) * common::patience());
        // SAFETY: the child's own pid, which it still is: it waits on CI.
        unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        let status = child.wait().unwrap();
        assert_eq!(status.code(), Some(130));
        assert_eq!(ci_branches(&bare), "", "deleted on an interrupt");
    }
}
