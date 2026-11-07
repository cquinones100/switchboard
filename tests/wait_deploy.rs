//! deploy-test's `switchboard wait-deploy`, against a small GitHub of the
//! test's own on 127.0.0.1 (`SWITCHBOARD_GITHUB_API` moved there, a dummy
//! token, a temporary HOME): a deployment that includes the merge commit
//! succeeding after a round in progress, a deploy workflow found by the name
//! it shows failing, nothing settling before the timeout, and the usage.

#![cfg(feature = "net")]

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use common::{BIN, TempDir};
use serde_json::{Value, json};

/// A GitHub answering each request by its path, through `route`, which is
/// given the path and how many times it has been asked for before.
struct Fake {
    url: String,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    fn start(route: impl Fn(&str, usize) -> (u16, Value) + Send + 'static) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let mut reader = BufReader::new(stream);
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let before = {
                    let mut log = log.lock().unwrap();
                    let before = log.iter().filter(|p| **p == path).count();
                    log.push(path.clone());
                    before
                };
                let (status, body) = route(&path, before);
                let body = body.to_string();
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let mut stream = reader.into_inner();
                let _ = stream.write_all(reply.as_bytes());
                let _ = stream.flush();
            }
        });
        Fake { url, seen }
    }
}

fn wait_deploy(fake: &Fake, args: &[&str]) -> Output {
    let home = TempDir::new("wait-deploy");
    Command::new(BIN)
        .arg("wait-deploy")
        .args(args)
        .env("HOME", home.path())
        .env("GH_TOKEN", "test-token")
        .env("SWITCHBOARD_GITHUB_API", &fake.url)
        .env("WAIT_DEPLOY_POLL_SECS", "0.05")
        .env_remove("GITHUB_TOKEN")
        .env_remove("SWITCHBOARD_SETTINGS")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn said(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_deployment_that_includes_the_commit_is_waited_for_until_it_succeeds() {
    let fake = Fake::start(|path, before| match path {
        "/repos/o/r/deployments?environment=Production%20web&per_page=15" => (
            200,
            json!([{ "id": 1, "sha": "old" }, { "id": 2, "sha": "later" }]),
        ),
        "/repos/o/r/compare/merged...later" => (200, json!({ "status": "ahead" })),
        "/repos/o/r/compare/merged...old" => (200, json!({ "status": "behind" })),
        "/repos/o/r/deployments/2/statuses?per_page=1" if before == 0 => {
            (200, json!([{ "state": "in_progress" }]))
        }
        "/repos/o/r/deployments/2/statuses?per_page=1" => (
            200,
            json!([{ "state": "success", "environment_url": "https://web.example" }]),
        ),
        _ => (404, json!({ "message": "Not Found" })),
    });
    let out = wait_deploy(
        &fake,
        &[
            "--repo",
            "o/r",
            "--sha",
            "merged",
            "--environment",
            "Production web",
        ],
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(said(&out), "deployed later https://web.example\n");
    let seen = fake.seen.lock().unwrap().clone();
    assert_eq!(
        seen.iter()
            .filter(|p| p.contains("compare/merged...later"))
            .count(),
        1,
        "whether a commit includes the merge is asked once: {seen:?}"
    );
    assert!(
        !seen.iter().any(|p| p.contains("deployments/1/")),
        "a deployment of an older commit is passed over"
    );
}

#[test]
fn a_workflow_named_as_it_shows_is_found_and_its_failure_said() {
    let fake = Fake::start(|path, _| match path {
        "/repos/o/r/actions/workflows/Deploy%20site/runs?branch=main&per_page=15" => {
            (404, json!({ "message": "Not Found" }))
        }
        "/repos/o/r/actions/workflows?per_page=100" => (
            200,
            json!({ "workflows": [{ "id": 7, "name": "CI" }, { "id": 77, "name": "Deploy site" }] }),
        ),
        "/repos/o/r/actions/workflows/77/runs?branch=main&per_page=15" => (
            200,
            json!({ "workflow_runs": [
                { "head_sha": "merged", "status": "completed", "conclusion": "failure", "html_url": "https://github.com/o/r/actions/runs/1" }
            ] }),
        ),
        "/repos/o/r/compare/merged...merged" => (200, json!({ "status": "identical" })),
        _ => (404, json!({ "message": "Not Found" })),
    });
    let out = wait_deploy(
        &fake,
        &[
            "--repo",
            "o/r",
            "--sha",
            "merged",
            "--workflow",
            "Deploy site",
            "--branch",
            "main",
        ],
    );
    assert_eq!(said(&out), "failed https://github.com/o/r/actions/runs/1\n");
}

#[test]
fn github_turned_off_in_settings_is_asked_still_and_a_refusal_ends_it_at_once() {
    let fake = Fake::start(|path, _| match path {
        "/repos/o/r/deployments?environment=prod&per_page=15" => {
            (200, json!([{ "id": 2, "sha": "merged" }]))
        }
        "/repos/o/r/compare/merged...merged" => (200, json!({ "status": "identical" })),
        "/repos/o/r/deployments/2/statuses?per_page=1" => (200, json!([{ "state": "success" }])),
        _ => (401, json!({ "message": "Bad credentials" })),
    });
    let home = TempDir::new("wait-deploy-off");
    let settings = home.path().join("settings.json");
    std::fs::write(&settings, "{\"github\": false}").unwrap();
    let run = |repo: &str| {
        Command::new(BIN)
            .args(["wait-deploy", "--repo", repo, "--sha", "merged"])
            .args(["--environment", "prod", "--timeout", "5"])
            .env("HOME", home.path())
            .env("GH_TOKEN", "test-token")
            .env("SWITCHBOARD_GITHUB_API", &fake.url)
            .env("SWITCHBOARD_SETTINGS", &settings)
            .env("WAIT_DEPLOY_POLL_SECS", "0.05")
            .env_remove("GITHUB_TOKEN")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    // A program you run asks GitHub whatever Settings says, as gh did.
    let out = run("o/r");
    assert_eq!(said(&out), "deployed merged -\n");
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
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
}

#[test]
fn nothing_settling_in_time_is_a_timeout_and_a_failed_request_is_retried() {
    let asked = Arc::new(AtomicUsize::new(0));
    let count = asked.clone();
    let fake = Fake::start(move |_, _| {
        count.fetch_add(1, Ordering::SeqCst);
        (500, json!({ "message": "oops" }))
    });
    let out = wait_deploy(
        &fake,
        &[
            "--repo",
            "o/r",
            "--sha",
            "merged",
            "--environment",
            "prod",
            "--timeout",
            "0.005",
        ],
    );
    assert_eq!(said(&out), "timeout\n");
    assert!(
        asked.load(Ordering::SeqCst) > 1,
        "asked again after a failure"
    );

    let usage = wait_deploy(&fake, &["--repo", "o/r", "--sha", "x", "--workflow", "D"]);
    assert_eq!(usage.status.code(), Some(2), "a workflow needs its branch");
}
