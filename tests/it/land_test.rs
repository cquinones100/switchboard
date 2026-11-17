//! queue-and-clean's `switchboard land-lock test`, in a worktree of a
//! temporary repository with a temporary HOME and repos.json: a stand-in
//! test command passing and failing, recorded in the landing's record with
//! the counts it printed, no command set or an empty one; and, with the
//! tests in CI, the landing's `ci/land-<id>` branch pushed to a bare
//! repository standing in for GitHub's (an `insteadOf` from its github.com
//! address) and deleted after a pass, a failure and an interrupt, against a
//! small GitHub of the test's own on 127.0.0.1, whose default branch's
//! run tells the change's failure from its own; and a commit tested again,
//! which waits for the run its own push started rather than answering from
//! an earlier one, takes an earlier run that passed with no push, and says
//! a canceled run as canceled.

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
    /// `runs` for every other, every commit with no statuses and no earlier
    /// run of GitHub Actions, and job 5's
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
            if request.target.contains("/actions/runs?head_sha=") {
                return Some(Reply::json(200, json!({ "workflow_runs": [] })));
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

    /// A GitHub for a commit tested again: `workflow_runs` are its GitHub
    /// Actions runs, and its check runs are `looks`' entry for how many
    /// times they were asked for before (the last from then on); the
    /// default branch's commit `base_sha` passes, and job 5's log fails
    /// `tabs::names` under nextest.
    fn github_again(workflow_runs: Value, base_sha: String, looks: Vec<Value>) -> http::Fake {
        github_with_jobs(workflow_runs, base_sha, looks, "success")
    }

    /// `github_again`, each run's one job ending as `job`.
    fn github_with_jobs(
        workflow_runs: Value,
        base_sha: String,
        looks: Vec<Value>,
        job: &'static str,
    ) -> http::Fake {
        let asked = std::sync::atomic::AtomicUsize::new(0);
        http::Fake::github("test-token", move |request| {
            if request.method != "GET" {
                return None;
            }
            if request.target == "/repos/o/r/actions/jobs/5/logs" {
                let log =
                    "2026-11-15T10:00:00.1Z         FAIL [   1.204s] switchboard::it tabs::names";
                return Some(Reply::json(200, Value::String(log.into())));
            }
            if request
                .target
                .starts_with("/repos/o/r/actions/runs?head_sha=")
            {
                return Some(Reply::json(200, json!({ "workflow_runs": workflow_runs })));
            }
            if request.target.starts_with("/repos/o/r/actions/runs/")
                && request.target.ends_with("/jobs?per_page=100")
            {
                let jobs = json!([{ "name": "Tests", "status": "completed", "conclusion": job }]);
                return Some(Reply::json(200, json!({ "jobs": jobs })));
            }
            if !request.target.starts_with("/repos/o/r/commits/") {
                return None;
            }
            let body = if !request.target.contains("/check-runs") {
                json!({ "state": "pending", "statuses": [] })
            } else if request.target.contains(&format!("/commits/{base_sha}/")) {
                json!({ "check_runs": [check(json!("success"), "completed")] })
            } else {
                let at = asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                json!({ "check_runs": looks[at.min(looks.len() - 1)] })
            };
            Some(Reply::json(200, body))
        })
    }

    /// A check run of the GitHub Actions run `run`, whose check suite is
    /// `run + 1000`.
    fn check_of(run: u64, conclusion: Value, status: &str) -> Value {
        json!({ "name": "Tests", "status": status, "conclusion": conclusion,
            "check_suite": { "id": run + 1000 },
            "details_url": format!("https://github.com/o/r/actions/runs/{run}/job/5") })
    }

    fn workflow_run(id: u64, conclusion: &str) -> Value {
        // The branch is `ci/land-` and git's hash of `feature`, the
        // worktree's branch, and a newline.
        json!({ "id": id, "workflow_id": 7, "check_suite_id": id + 1000,
            "event": "push", "head_branch": "ci/land-a7453f07505c",
            "status": "completed", "conclusion": conclusion })
    }

    #[test]
    fn a_commit_tested_again_waits_for_the_run_its_own_push_started() {
        let tmp = TempDir::new("land-test-again");
        let home = tmp.path();
        let dir = worktree(home);
        let bare = origin(home, &dir);
        repos(home, "o/r", json!({ "landing": "direct", "testIn": "ci" }));
        let head = git(&dir, &["rev-parse", "HEAD"]);
        let main = git(&dir, &["rev-parse", "main"]);
        // Run 77 failed and run 78 was canceled before this push, whose own
        // run, 79, registers only at the third look.
        let earlier = json!([workflow_run(77, "failure"), workflow_run(78, "cancelled")]);
        let before = vec![
            check_of(77, json!("failure"), "completed"),
            check_of(78, json!("cancelled"), "completed"),
        ];
        let with = |own: Value| {
            let mut all = before.clone();
            all.push(own);
            Value::Array(all)
        };
        let fake = github_again(
            earlier.clone(),
            main.clone(),
            vec![
                Value::Array(before.clone()),
                Value::Array(before.clone()),
                with(check_of(79, Value::Null, "in_progress")),
                with(check_of(79, json!("success"), "completed")),
            ],
        );
        let passed = in_ci(home, &dir, &fake).output().unwrap();
        assert_eq!(
            said(&passed),
            "passed\n",
            "the earlier runs' failure and canceled job aren't the answer"
        );
        assert_eq!(passed.status.code(), Some(0));
        assert_eq!(record(&dir)["tests"]["ok"], true);
        assert_eq!(ci_branches(&bare), "", "deleted after the pass");

        let fake = github_again(
            earlier.clone(),
            main.clone(),
            vec![with(check_of(79, json!("failure"), "completed"))],
        );
        let failed = in_ci(home, &dir, &fake).output().unwrap();
        assert_eq!(
            said(&failed),
            format!("failed in CI {head} 79 Tests\n"),
            "the new run's failure, named by its own run"
        );

        let fake = github_again(
            earlier,
            main,
            vec![with(check_of(79, json!("cancelled"), "completed"))],
        );
        let canceled = in_ci(home, &dir, &fake).output().unwrap();
        assert_eq!(
            said(&canceled),
            format!("canceled in CI {head} 79 Tests\n"),
            "a canceled run is said as canceled, not failed"
        );
        assert_eq!(canceled.status.code(), Some(8));
        assert_eq!(ci_branches(&bare), "", "deleted after a canceled run");
    }

    #[test]
    fn a_commit_whose_earlier_run_passed_is_not_pushed_again() {
        let tmp = TempDir::new("land-test-before");
        let home = tmp.path();
        let dir = worktree(home);
        let bare = origin(home, &dir);
        repos(home, "o/r", json!({ "landing": "direct", "testIn": "ci" }));
        let head = git(&dir, &["rev-parse", "HEAD"]);
        let main = git(&dir, &["rev-parse", "main"]);
        // Run 77's failed job was run again and passed, which GitHub gives
        // as the run's conclusion; run 78 was canceled.
        let fake = github_again(
            json!([workflow_run(77, "success"), workflow_run(78, "cancelled")]),
            main,
            vec![json!([
                check_of(77, json!("success"), "completed"),
                check_of(78, json!("cancelled"), "completed")
            ])],
        );
        let passed = in_ci(home, &dir, &fake).output().unwrap();
        assert_eq!(said(&passed), "passed in CI before, run 77\npassed\n");
        assert_eq!(passed.status.code(), Some(0));
        assert_eq!(record(&dir)["tests"]["ok"], true);
        assert_eq!(record(&dir)["tests"]["head"], head);
        let pushed = Command::new("git")
            .args(["cat-file", "-e", &head])
            .current_dir(&bare)
            .status()
            .unwrap();
        assert!(!pushed.success(), "nothing was pushed");

        // A run that succeeded with every job skipped ran no test, so the
        // commit is pushed and its new run waited for.
        let fake = github_with_jobs(
            json!([workflow_run(77, "success")]),
            git(&dir, &["rev-parse", "main"]),
            vec![json!([
                check_of(77, json!("skipped"), "completed"),
                check_of(79, json!("failure"), "completed")
            ])],
            "skipped",
        );
        let skipped = in_ci(home, &dir, &fake).output().unwrap();
        assert_eq!(
            said(&skipped),
            format!("failed in CI {head} 79 Tests\n"),
            "no job of the earlier run succeeded"
        );

        // Another CI's check failing on the commit: the Actions pass alone
        // is no answer, so it is pushed and waited for.
        let elsewhere = json!({ "name": "other-ci", "status": "completed",
            "conclusion": "failure", "details_url": "https://ci.example/build/9" });
        let fake = github_again(
            json!([workflow_run(77, "success")]),
            git(&dir, &["rev-parse", "main"]),
            vec![json!([
                check_of(77, json!("success"), "completed"),
                elsewhere
            ])],
        );
        let failed = in_ci(home, &dir, &fake).output().unwrap();
        assert_eq!(
            said(&failed).lines().last(),
            Some(format!("mixed in CI {head} - other-ci").as_str()),
            "{}",
            said(&failed)
        );
    }

    #[test]
    fn github_not_answering_for_the_commit_s_earlier_runs_runs_the_tests_here() {
        let tmp = TempDir::new("land-test-unknown");
        let home = tmp.path();
        let dir = worktree(home);
        let bare = origin(home, &dir);
        repos(
            home,
            "o/r",
            json!({ "landing": "direct", "test": "true", "testIn": "ci" }),
        );
        let failing = json!([check(json!("failure"), "completed")]);
        let fake = http::Fake::github("test-token", move |request| {
            if request.target.contains("/actions/runs?head_sha=") {
                return Some(Reply::json(502, json!({ "message": "Bad gateway" })));
            }
            let body = if request.target.contains("/check-runs") {
                json!({ "check_runs": failing })
            } else {
                json!({ "state": "pending", "statuses": [] })
            };
            Some(Reply::json(200, body))
        });
        let here = in_ci(home, &dir, &fake).output().unwrap();
        assert_eq!(
            said(&here).lines().last(),
            Some("passed"),
            "never answered from check runs it can't tell apart: {}",
            said(&here)
        );
        assert!(said(&here).contains("running the tests here"));
        assert_eq!(ci_branches(&bare), "", "nothing pushed");
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
