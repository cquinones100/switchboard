//! queue-and-clean's `switchboard wait-ci`, against a small GitHub of the
//! test's own on 127.0.0.1 (`SWITCHBOARD_GITHUB_API` moved there, a dummy
//! token, a temporary HOME): a commit whose check runs finish one by one
//! passing, a failing job named with its run, no check run within the grace
//! period, nothing settling before the timeout, and the usage.

#![cfg(feature = "net")]

use crate::common;

use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::http::{self, Reply};
use common::{BIN, TempDir, stand_in};
use serde_json::{Value, json};

const CHECK_RUNS: &str = "/repos/o/r/commits/abc123/check-runs?per_page=100";
const STATUSES: &str = "/repos/o/r/commits/abc123/status?per_page=100";

/// A commit with no statuses, as one whose CI makes check runs alone.
fn no_statuses() -> (u16, Value) {
    (200, json!({ "state": "pending", "statuses": [] }))
}

/// A GitHub as strict as GitHub about how it is asked (tests/common/http.rs:
/// a User-Agent, the token `test-token`), answering each GET by its path and
/// query, through `route`, which is given them and how many times they have
/// been asked for before.
struct Fake {
    url: String,
    seen: Arc<Mutex<Vec<String>>>,
    _github: http::Fake,
}

impl Fake {
    fn start(route: impl Fn(&str, usize) -> (u16, Value) + Send + Sync + 'static) -> Fake {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        let github = http::Fake::github("test-token", move |request| {
            if request.method != "GET" {
                return None;
            }
            let path = request.target.clone();
            let before = {
                let mut log = log.lock().unwrap();
                let before = log.iter().filter(|p| **p == path).count();
                log.push(path.clone());
                before
            };
            let (status, body) = route(&path, before);
            Some(Reply::json(status, body))
        });
        Fake {
            url: github.url.clone(),
            seen,
            _github: github,
        }
    }
}

fn wait_ci(fake: &Fake, args: &[&str]) -> Output {
    let home = TempDir::new("wait-ci");
    Command::new(BIN)
        .arg("wait-ci")
        .args(args)
        .env("HOME", home.path())
        .env("PATH", stand_in::path_with_gh(&home.path().join("bin")))
        .env("GH_TOKEN", "test-token")
        .env("SWITCHBOARD_GITHUB_API", &fake.url)
        .env("WAIT_CI_POLL_SECS", "0.05")
        .env_remove("GITHUB_TOKEN")
        .env_remove("SWITCHBOARD_SETTINGS")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn said(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn check(name: &str, status: &str, conclusion: Value) -> Value {
    json!({
        "name": name,
        "status": status,
        "conclusion": conclusion,
        "details_url": "https://github.com/o/r/actions/runs/77/job/5",
    })
}

fn runs(check_runs: Vec<Value>) -> (u16, Value) {
    (
        200,
        json!({ "total_count": check_runs.len(), "check_runs": check_runs }),
    )
}

#[test]
fn a_commit_whose_check_runs_finish_one_by_one_is_waited_for_and_said_passed() {
    let fake = Fake::start(|path, before| match (path, before) {
        (STATUSES, _) => no_statuses(),
        (CHECK_RUNS, 0) => runs(Vec::new()),
        (CHECK_RUNS, 1) => runs(vec![
            check("Formatting", "completed", json!("success")),
            check("Tests", "in_progress", Value::Null),
        ]),
        (CHECK_RUNS, _) => runs(vec![
            check("Formatting", "completed", json!("success")),
            check("Tests", "completed", json!("success")),
            check("Docs", "completed", json!("skipped")),
        ]),
        _ => (404, json!({ "message": "Not Found" })),
    });
    let out = wait_ci(&fake, &["--repo", "o/r", "--sha", "abc123"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(said(&out), "passed abc123\n");
    assert_eq!(
        fake.seen.lock().unwrap().len(),
        8,
        "passing holds on two looks in a row, for a job made late"
    );
}

#[test]
fn a_failing_job_is_named_with_its_run_while_the_others_still_run() {
    let fake = Fake::start(|path, _| match path {
        STATUSES => no_statuses(),
        CHECK_RUNS => runs(vec![
            check("Formatting", "completed", json!("success")),
            check("Clippy and tests (Linux)", "completed", json!("failure")),
            check("Clippy and tests (macOS)", "in_progress", Value::Null),
        ]),
        _ => (404, json!({ "message": "Not Found" })),
    });
    let out = wait_ci(&fake, &["--repo", "o/r", "--sha", "abc123"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(said(&out), "failed abc123 77 Clippy and tests (Linux)\n");
    assert_eq!(fake.seen.lock().unwrap().len(), 2, "said at the first look");
}

#[test]
fn a_commit_status_is_waited_for_and_its_failure_said_as_a_check_run_s_is() {
    let fake = Fake::start(|path, before| match (path, before) {
        (CHECK_RUNS, _) => runs(vec![check("Formatting", "completed", json!("success"))]),
        (STATUSES, 0 | 1) => (
            200,
            json!({ "state": "pending", "statuses": [{ "context": "ci/tests", "state": "pending" }] }),
        ),
        (STATUSES, _) => (
            200,
            json!({ "state": "failure", "statuses": [{ "context": "ci/tests", "state": "failure" }] }),
        ),
        _ => (404, json!({ "message": "Not Found" })),
    });
    let out = wait_ci(&fake, &["--repo", "o/r", "--sha", "abc123"]);
    assert_eq!(
        said(&out),
        "failed abc123 - ci/tests\n",
        "a passing check run beside a pending status is not a pass"
    );
}

#[test]
fn no_check_run_within_the_grace_period_is_none() {
    let fake = Fake::start(|path, _| match path {
        STATUSES => no_statuses(),
        CHECK_RUNS => runs(Vec::new()),
        _ => (404, json!({ "message": "Not Found" })),
    });
    let out = wait_ci(
        &fake,
        &["--repo", "o/r", "--sha", "abc123", "--grace", "0.005"],
    );
    assert_eq!(said(&out), "none\n");
    assert!(
        fake.seen.lock().unwrap().len() > 1,
        "asked again within the grace period"
    );
}

#[test]
fn github_turned_off_in_settings_is_asked_still_and_a_refusal_ends_it_at_once() {
    let fake = Fake::start(|path, _| match path {
        STATUSES => no_statuses(),
        CHECK_RUNS => runs(vec![check("Tests", "completed", json!("success"))]),
        _ => (401, json!({ "message": "Bad credentials" })),
    });
    let home = TempDir::new("wait-ci-off");
    let settings = home.path().join("settings.json");
    std::fs::write(&settings, "{\"github\": false}").unwrap();
    let run = |repo: &str| {
        Command::new(BIN)
            .args(["wait-ci", "--repo", repo, "--sha", "abc123"])
            .args(["--timeout", "5"])
            .env("HOME", home.path())
            .env("PATH", stand_in::path_with_gh(&home.path().join("bin")))
            .env("GH_TOKEN", "test-token")
            .env("SWITCHBOARD_GITHUB_API", &fake.url)
            .env("SWITCHBOARD_SETTINGS", &settings)
            .env("WAIT_CI_POLL_SECS", "0.05")
            .env_remove("GITHUB_TOKEN")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    // A program you run asks GitHub whatever Settings says, as gh did.
    let out = run("o/r");
    assert_eq!(said(&out), "passed abc123\n");
    // A token GitHub refuses won't mend by waiting: said at once, exit 1.
    let started = std::time::Instant::now();
    let refused = run("o/other");
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(said(&refused), "");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("refused the token"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(4) * common::patience());
}

#[test]
fn nothing_settling_in_time_is_a_timeout_and_a_failed_request_is_never_none() {
    let asked = Arc::new(AtomicUsize::new(0));
    let count = asked.clone();
    let fake = Fake::start(move |_, _| {
        count.fetch_add(1, Ordering::SeqCst);
        (500, json!({ "message": "oops" }))
    });
    let out = wait_ci(
        &fake,
        &[
            "--repo",
            "o/r",
            "--sha",
            "abc123",
            "--grace",
            "0",
            "--timeout",
            "0.005",
        ],
    );
    assert_eq!(
        said(&out),
        "timeout\n",
        "a request that failed says nothing of whether CI runs"
    );
    assert!(
        asked.load(Ordering::SeqCst) > 1,
        "asked again after a failure"
    );

    let usage = wait_ci(&fake, &["--repo", "o/r"]);
    assert_eq!(usage.status.code(), Some(2), "a commit is needed");
}

#[test]
fn now_says_what_one_look_sees_passed_failed_running_or_none() {
    let fake = Fake::start(|path, _| match path {
        STATUSES => no_statuses(),
        CHECK_RUNS => runs(vec![
            check("Formatting", "completed", json!("success")),
            check("Tests", "completed", json!("success")),
        ]),
        _ => (404, json!({ "message": "Not Found" })),
    });
    let now = |fake: &Fake| wait_ci(fake, &["--now", "--repo", "o/r", "--sha", "abc123"]);
    let out = now(&fake);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(said(&out), "passed abc123\n", "a pass at one look");
    assert_eq!(fake.seen.lock().unwrap().len(), 2, "one look, no waiting");

    let fake = Fake::start(|path, _| match path {
        STATUSES => no_statuses(),
        CHECK_RUNS => runs(vec![
            check("Clippy and tests (macOS)", "completed", json!("failure")),
            check("Clippy and tests (Linux)", "in_progress", Value::Null),
        ]),
        _ => (404, json!({ "message": "Not Found" })),
    });
    assert_eq!(
        said(&now(&fake)),
        "failed abc123 77 Clippy and tests (macOS)\n"
    );

    let fake = Fake::start(|path, _| match path {
        STATUSES => no_statuses(),
        CHECK_RUNS => runs(vec![check("Tests", "queued", Value::Null)]),
        _ => (404, json!({ "message": "Not Found" })),
    });
    assert_eq!(said(&now(&fake)), "running abc123\n");

    let fake = Fake::start(|path, _| match path {
        STATUSES => no_statuses(),
        CHECK_RUNS => runs(Vec::new()),
        _ => (404, json!({ "message": "Not Found" })),
    });
    assert_eq!(said(&now(&fake)), "none\n");
    assert_eq!(fake.seen.lock().unwrap().len(), 2);
}
