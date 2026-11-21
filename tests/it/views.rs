//! The views as someone uses them, against a server of the test's own and a
//! stand-in for GitHub on 127.0.0.1 (as tests/tabs.rs has one): M-v opens
//! the branch's PR description in a pane to the right, running `switchboard
//! pr-view`, and M-v again closes it; M-r where there is no repository says
//! why it can't open the PR. And `switchboard open-url` opens a link in its
//! site's Chrome profile, with a stand-in for Chrome writing down what it
//! was given (test/open-url.test.js's test, with its `open`). A link a
//! program in a pane writes as OSC 8 reaches the client as one, and M-p's
//! link commands list a pane's links and images, copying one, reading one
//! beside it, or opening a file through a stand-in for the desktop's opener.

use crate::common;

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{BIN, TempDir, stand_in};
use serde_json::json;
// What only the tests against a stand-in for GitHub use, which need the
// `net` feature.
use {common::TestServer, serde_json::Value, std::path::Path};

/// Settings for the tests: no desktop notifications or tips, and these.
fn settings(server: &TestServer, extra: Value) {
    common::write_settings(&server.home, all_settings(extra));
}

/// The settings `settings` writes, `extra` over them.
fn all_settings(extra: Value) -> Value {
    let mut all = json!({"notifications": false, "tips": false});
    for (k, v) in extra.as_object().unwrap() {
        all[k] = v.clone();
    }
    all
}

/// A server started with what `settings` writes, so they hold from its
/// first draw.
fn start(extra: Value) -> TestServer {
    TestServer::start_with_settings(&all_settings(extra).to_string())
}

/// Runs git in `dir` with an identity of its own.
#[cfg(feature = "net")]
fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// The first tab's panes, as `list` gives them.
fn panes(server: &TestServer) -> Vec<Value> {
    server.list()["sessions"][0]["tabs"][0]["panes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn wait_panes(server: &TestServer, n: usize) -> Vec<Value> {
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    loop {
        let found = panes(server);
        if found.len() == n {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "waiting for {n} panes: {found:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Closes the message box saying `text` with x, and waits until it has
/// gone: a command sent next reaches the server by another way than the
/// client's keys, and could otherwise open its list first, taking the x as
/// what is typed into it.
fn close_message(client: &mut common::TestClient, text: &str) {
    client.type_text("x");
    client.wait_for("the message closed", |s| {
        !s.iter().any(|l| l.contains(text))
    });
}

#[cfg(feature = "net")]
mod with_github {
    use super::*;
    use crate::common::http::{Fake, Reply};
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A stand-in for GitHub (tests/common/http.rs), as strict as GitHub
    /// about how it is asked: the PR on a branch for a question about pull
    /// requests, its description for one about a pull request by its number.
    fn fake_github() -> Fake {
        Fake::github("test-token", |request| {
            if request.path() != "/graphql" {
                return None;
            }
            let asked = request.text();
            let pr = json!({
                "number": 12, "url": "https://github.com/acme/widgets/pull/12",
                "title": "Fix the login", "state": "OPEN", "isDraft": false,
                "baseRefName": "main", "headRefName": "feature-x",
                "headRepositoryOwner": {"login": "acme"}, "author": {"login": "jo"},
                "commits": {"totalCount": 2}, "changedFiles": 1,
                "files": {"nodes": [{"path": "src/login.rs", "additions": 3, "deletions": 1}]},
                "body": "## Why\n\nThe token went **stale**.\n\n```mermaid\nflowchart LR\n  A[Login] --> B[Token]\n```\n"
            });
            let answer = if asked.contains("pullRequests") {
                json!({"data": {"repository": {"pullRequests": {"nodes": [pr]}}}})
            } else {
                json!({"data": {"repository": {"pullRequest": pr}}})
            };
            Some(Reply::json(200, answer))
        })
    }

    #[test]
    fn m_v_opens_the_pr_and_closes_it_again() {
        let fake = fake_github();
        let server = TestServer::start_with_env_and_settings(
            &[
                ("GH_TOKEN", "test-token"),
                ("SWITCHBOARD_GITHUB_GRAPHQL", &fake.graphql_url()),
                ("SWITCHBOARD_GITHUB_API", &fake.url),
            ],
            &all_settings(json!({"linear": false})).to_string(),
        );
        let work = server.dir.path().join("widgets");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q"]);
        git(&work, &["symbolic-ref", "HEAD", "refs/heads/feature-x"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widgets.git",
            ],
        );

        let mut client = server.attach(140, 36);
        client.wait_text("$");

        // Outside a repository, M-r says why it can't open the PR.
        client.send(b"\x1br");
        client.wait_text("Can't open the PR");
        client.wait_text("isn't in a git repository");
        client.type_text("x");

        client.type_text(&format!("cd {}\r", work.display()));
        let tab = server.list()["sessions"][0]["tabs"][0].clone();
        let deadline = common::deadline(std::time::Duration::from_secs(10));
        while !panes(&server)[0]["cwd"]
            .as_str()
            .is_some_and(|d| d.ends_with("widgets"))
        {
            assert!(Instant::now() < deadline, "the shell didn't cd: {tab}");
            std::thread::sleep(Duration::from_millis(50));
        }

        client.send(b"\x1bv");
        client.wait_text("#12 Fix the login");
        client.wait_text("jo wants to merge");
        // Kept for the next time, at once.
        let cache = server.home.join(".cache/switchboard/pr-view");
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);

        client.send(b"\x1bv");
        // The view's own words, since the tab is named by the PR's title.
        client.wait_for("the view gone", |s| {
            !s.iter().any(|l| l.contains("jo wants to merge"))
        });
    }

    #[test]
    fn m_v_shows_the_pr_s_board_in_claude_s_place() {
        let fake = fake_github();
        // Set from the start, so M-v never finds the server still without it.
        let server = TestServer::start_with_env_and_settings(
            &[
                ("GH_TOKEN", "test-token"),
                ("SWITCHBOARD_GITHUB_GRAPHQL", &fake.graphql_url()),
                ("SWITCHBOARD_GITHUB_API", &fake.url),
            ],
            &json!({"notifications": false, "tips": false, "linear": false, "sidebar": false, "photos": false}).to_string(),
        );
        let work = server.dir.path().join("widgets");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q"]);
        git(&work, &["symbolic-ref", "HEAD", "refs/heads/feature-x"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widgets.git",
            ],
        );
        let mut client = server.attach(140, 36);
        client.wait_for("the header", |s| s[2].contains("M-c Claude"));
        client.wait_text("$");
        client.type_text(&format!("cd {}\r", work.display()));
        let deadline = common::deadline(std::time::Duration::from_secs(10));
        while !panes(&server)[0]["cwd"]
            .as_str()
            .is_some_and(|d| d.ends_with("widgets"))
        {
            assert!(Instant::now() < deadline, "the shell didn't cd");
            std::thread::sleep(Duration::from_millis(50));
        }

        client.send(b"\x1bv");
        // The board: its head, the description, and the panel beside it
        // with the PR's files, the footer's keys under them.
        let screen = client.wait_text("FILES");
        // The board's head, not the tab bar, which the PR's title names too.
        let head = screen
            .iter()
            .find(|l| l.contains("#12 Fix the login") && !l.contains(" 1 #12"));
        assert!(head.is_some_and(|h| h.contains("Open")), "{screen:#?}");
        client.wait_text("jo wants to merge 2 commits into main from feature-x");
        let file = client.wait_text("src/login.rs");
        let row = file.iter().find(|l| l.contains("src/login.rs")).unwrap();
        assert!(
            row.contains('│') && row.trim_end().ends_with("+3 −1"),
            "{row:?}"
        );
        client.wait_text("M-r on GitHub");
        // In Claude's place: the only pane, the header's PR key lit.
        let shown = wait_panes(&server, 1);
        assert_eq!(shown[0]["options"]["@pr_view"], "1", "{shown:?}");

        client.send(b"\x1bv");
        client.wait_for("the board gone", |s| !s.iter().any(|l| l.contains("FILES")));
    }

    /// A stand-in for GitHub whose PR has one check, `build`, failing until
    /// `green`, answering the view's questions and the tabs' PR lookup
    /// (its aliased query), and nothing at all while `down`.
    fn changing_github(green: Arc<AtomicBool>, down: Arc<AtomicBool>) -> Fake {
        Fake::github("test-token", move |request| {
            if request.path() != "/graphql" {
                return None;
            }
            if down.load(Ordering::SeqCst) {
                return Some(Reply::json(500, json!({"message": "down"})));
            }
            let asked = request.text();
            let conclusion = if green.load(Ordering::SeqCst) {
                "SUCCESS"
            } else {
                "FAILURE"
            };
            let contexts = json!({
                "checkRunCountsByState": [{"state": conclusion, "count": 1}],
                "statusContextCountsByState": [],
                "nodes": [{"__typename": "CheckRun", "name": "build", "status": "COMPLETED", "conclusion": conclusion}]
            });
            let commit = json!({"nodes": [{"commit": {"statusCheckRollup": {"state": conclusion, "contexts": contexts}}}]});
            let mut commits = commit.clone();
            commits["totalCount"] = json!(2);
            let pr = json!({
                "number": 12, "url": "https://github.com/acme/widgets/pull/12",
                "title": "Fix the login", "state": "OPEN", "isDraft": false,
                "baseRefName": "main", "headRefName": "feature-x",
                "headRepositoryOwner": {"login": "acme"}, "author": {"login": "jo"},
                "repository": {"nameWithOwner": "acme/widgets"},
                "additions": 3, "deletions": 1, "mergedAt": null, "mergeQueueEntry": null,
                "commits": commits, "last": commit, "changedFiles": 1,
                "files": {"nodes": [{"path": "src/login.rs", "additions": 3, "deletions": 1}]},
                "body": "The token went stale."
            });
            let answer = if asked.contains("r0:") && asked.contains("pullRequest(number:") {
                json!({"data": {"r0": {"a0": pr}}})
            } else if asked.contains("r0:") {
                json!({"data": {"r0": {"a0": {"nodes": [pr]}}}})
            } else if asked.contains("pullRequests") {
                json!({"data": {"repository": {"pullRequests": {"nodes": [pr]}}}})
            } else {
                json!({"data": {"repository": {"pullRequest": pr}}})
            };
            Some(Reply::json(200, answer))
        })
    }

    #[test]
    fn an_open_pr_view_follows_the_pr_as_the_lookups_see_it_change() {
        let green = Arc::new(AtomicBool::new(false));
        let down = Arc::new(AtomicBool::new(false));
        let fake = changing_github(green.clone(), down.clone());
        // poll_fast off, so the PR is asked about again within its minute
        // only when something asks for it.
        let server = TestServer::start_with_env_and_settings(
            &[
                ("GH_TOKEN", "test-token"),
                ("SWITCHBOARD_GITHUB_GRAPHQL", &fake.graphql_url()),
                ("SWITCHBOARD_GITHUB_API", &fake.url),
            ],
            &json!({"notifications": false, "tips": false, "linear": false, "sidebar": false,
                "photos": false, "pause_when_away": false, "poll_fast": false})
            .to_string(),
        );
        let work = server.dir.path().join("widgets");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q"]);
        git(&work, &["symbolic-ref", "HEAD", "refs/heads/feature-x"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widgets.git",
            ],
        );
        let mut client = server.attach(140, 36);
        client.wait_for("the header", |s| s[2].contains("M-c Claude"));
        client.wait_text("$");
        client.type_text(&format!("cd {}\r", work.display()));
        let fails = |server: &TestServer| {
            server.list()["sessions"][0]["tabs"][0]["facts"]["pr"]["checks"]["fail"].clone()
        };
        let wait_fails = |n: u64| {
            let deadline = common::deadline(Duration::from_secs(30));
            while fails(&server) != n {
                assert!(
                    Instant::now() < deadline,
                    "the lookup's checks: {n} failing"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        };
        wait_fails(1);

        client.send(b"\x1bv");
        client.wait_text("✕ build");
        let failing = |s: &[String]| s.iter().any(|l| l.contains("✕ build"));
        let checks = |s: &[String], count: &str| {
            s.iter()
                .any(|l| l.contains("CHECKS") && l.trim_end().ends_with(count))
        };
        client.wait_for("the failing check counted", |s| checks(s, "0 of 1"));
        client.wait_text("r read again");

        // The check passes, and something that just asked GitHub says so in
        // the github file: the lookup sees it, and the view, left open,
        // shows it.
        let asked_github = || {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(server.state.join("github"))
                .unwrap()
                .write_all(b"\n")
                .unwrap();
        };
        green.store(true, Ordering::SeqCst);
        asked_github();
        client.wait_for("the check passing in the open view", |s| {
            !failing(s) && checks(s, "1 of 1")
        });

        // It fails again with no lookup to see it: r reads it at once.
        wait_fails(0);
        green.store(false, Ordering::SeqCst);
        client.type_text("r");
        client.wait_for("r read it again", |s| failing(s) && checks(s, "0 of 1"));

        // With GitHub not answering, a view opened on a copy kept two hours
        // ago shows it, and says how old it is.
        client.send(b"\x1bv");
        client.wait_for("the board gone", |s| !s.iter().any(|l| l.contains("FILES")));
        down.store(true, Ordering::SeqCst);
        let cache = server.home.join(".cache/switchboard/pr-view");
        let copy = std::fs::read_dir(&cache).unwrap().next().unwrap().unwrap();
        std::fs::File::options()
            .write(true)
            .open(copy.path())
            .unwrap()
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(2 * 3600))
            .unwrap();
        client.send(b"\x1bv");
        client.wait_text("✕ build");
        client.wait_text("read 2h ago");
    }

    /// A stand-in for GitHub with a PR whose first hundred checks all pass,
    /// a failing one coming on the second page, and whose first hundred
    /// review threads are all resolved, an open one on the second page:
    /// each second page answered only for the first page's cursor.
    fn paged_github() -> Fake {
        Fake::github("test-token", |request| {
            if request.path() != "/graphql" {
                return None;
            }
            let body = request.json();
            let asked = body["query"].as_str().unwrap_or("").to_string();
            let after = body["variables"]["after"].as_str().map(str::to_string);
            let pr = json!({
                "number": 12, "url": "https://github.com/acme/widgets/pull/12",
                "title": "Fix the login", "state": "OPEN", "isDraft": false,
                "baseRefName": "main", "headRefName": "feature-x",
                "headRepositoryOwner": {"login": "acme"}, "author": {"login": "jo"},
                "commits": {"totalCount": 2}, "changedFiles": 0, "body": "Why."
            });
            let in_pr = |pr: Value| json!({"data": {"repository": {"pullRequest": pr}}});
            let page = |nodes: Vec<Value>, next: Option<&str>| json!({"pageInfo": {"hasNextPage": next.is_some(), "endCursor": next}, "nodes": nodes});
            let checks_page = |after: Option<&str>| {
                let mut checks = match after {
                    None => page(
                        (0..100)
                            .map(|i| json!({"__typename": "CheckRun", "name": format!("job {i}"), "status": "COMPLETED", "conclusion": "SUCCESS", "isRequired": true}))
                            .collect(),
                        Some("checks-1"),
                    ),
                    Some("checks-1") => page(
                        vec![json!({"__typename": "CheckRun", "name": "late e2e", "status": "COMPLETED", "conclusion": "FAILURE", "isRequired": true, "detailsUrl": "https://ci.example/9"})],
                        None,
                    ),
                    Some(other) => panic!("asked after {other}"),
                };
                checks["checkRunCountsByState"] =
                    json!([{"state": "SUCCESS", "count": 100}, {"state": "FAILURE", "count": 1}]);
                checks["statusContextCountsByState"] = json!([]);
                json!({"nodes": [{"commit": {"statusCheckRollup": {"contexts": checks}}}]})
            };
            let answer = if asked.contains("pullRequests") {
                json!({"data": {"repository": {"pullRequests": {"nodes": [pr]}}}})
            } else if asked.contains("statusCheckRollup") && !asked.contains("files(") {
                // The checks after the view's first page, and --checks'.
                let mut pr = pr;
                pr["commits"] = checks_page(after.as_deref());
                in_pr(pr)
            } else if asked.contains("reviewThreads") {
                let thread = |open: bool, path: &str| {
                    json!({"isResolved": !open, "path": path, "line": 3,
                        "comments": {"totalCount": 1, "nodes": [{"author": {"login": "ana"}, "body": "Look here."}]},
                        "latest": {"nodes": [{"author": {"login": "ana"}, "body": "Look here."}]}})
                };
                let threads = match after.as_deref() {
                    None => page(
                        (0..100)
                            .map(|i| thread(false, &format!("done{i}.rs")))
                            .collect(),
                        Some("threads-1"),
                    ),
                    Some("threads-1") => page(vec![thread(true, "src/late.rs")], None),
                    Some(other) => panic!("asked after {other}"),
                };
                let mut pr = if asked.contains("files(") {
                    let mut pr = pr;
                    pr["mergeable"] = json!("MERGEABLE");
                    pr["mergeStateStatus"] = json!("BLOCKED");
                    pr["reviewDecision"] = json!("APPROVED");
                    pr["last"] = checks_page(None);
                    pr
                } else {
                    json!({})
                };
                pr["reviewThreads"] = threads;
                in_pr(pr)
            } else {
                in_pr(pr)
            };
            Some(Reply::json(200, answer))
        })
    }

    /// A repository on feature-x with origin on GitHub, in the test's home.
    fn widgets(server: &TestServer) -> std::path::PathBuf {
        let work = server.dir.path().join("widgets");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q"]);
        git(&work, &["symbolic-ref", "HEAD", "refs/heads/feature-x"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widgets.git",
            ],
        );
        work
    }

    fn cd(server: &TestServer, client: &mut common::TestClient, dir: &Path) {
        client.type_text(&format!("cd {}\r", dir.display()));
        let deadline = common::deadline(std::time::Duration::from_secs(10));
        while !panes(server)[0]["cwd"]
            .as_str()
            .is_some_and(|d| d.ends_with("widgets"))
        {
            assert!(Instant::now() < deadline, "the shell didn't cd");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// A server with UX 2.0 on, asking `fake` for GitHub.
    fn board_server(fake: &Fake) -> TestServer {
        TestServer::start_with_env_and_settings(
            &[
                ("GH_TOKEN", "test-token"),
                ("SWITCHBOARD_GITHUB_GRAPHQL", &fake.graphql_url()),
                ("SWITCHBOARD_GITHUB_API", &fake.url),
            ],
            &json!({"notifications": false, "tips": false, "linear": false, "sidebar": false, "photos": false}).to_string(),
        )
    }

    #[test]
    fn m_v_counts_a_required_check_failing_past_the_first_page_and_shows_the_failing_ones() {
        let fake = paged_github();
        let server = board_server(&fake);
        let work = widgets(&server);
        let mut client = server.attach(140, 36);
        client.wait_for("the header", |s| s[2].contains("M-c Claude"));
        client.wait_text("$");
        cd(&server, &mut client, &work);

        client.send(b"\x1bv");
        client.wait_text("✕ Blocked");
        client.wait_text("required 100 of 101 passed · approved");
        // m opens the view's menu, on what it offers now.
        client.type_text("m");
        client.wait_text("Read it again");
        client.wait_text("› See the failing checks");
        client.send(b"\r");
        client.wait_text("Failing checks of #12");
        client.wait_text("late e2e");
        client.wait_text("https://ci.example/9");
    }

    /// A stand-in for GitHub with no PR on any branch, and feature-x's
    /// commit failing a check whose job's log ends in the failure.
    fn branch_github() -> Fake {
        Fake::github("test-token", |request| {
            if request.target == "/repos/acme/widgets/actions/jobs/5/logs" {
                return Some(Reply::bytes(
                    200,
                    "text/plain",
                    "2026-11-11T10:00:00.0000000Z running\n2026-11-11T10:00:01.0000000Z FAIL login::token\n",
                ));
            }
            if request.path() != "/graphql" {
                return None;
            }
            let body = request.json();
            let asked = body["query"].as_str().unwrap_or("");
            let answer = if asked.contains("pullRequests") {
                json!({"data": {"repository": {"pullRequests": {"nodes": []}}}})
            } else if asked.contains("ref(qualifiedName") {
                assert_eq!(body["variables"]["qualified"], "refs/heads/feature-x");
                json!({"data": {"repository": {"ref": {"target": {
                    "oid": "0123456789abcdef0123456789abcdef01234567",
                    "statusCheckRollup": {"contexts": {
                        "checkRunCountsByState": [{"state": "SUCCESS", "count": 2}, {"state": "FAILURE", "count": 1}],
                        "statusContextCountsByState": [],
                        "pageInfo": {"hasNextPage": false, "endCursor": null},
                        "nodes": [
                            {"__typename": "CheckRun", "name": "lint", "status": "COMPLETED", "conclusion": "SUCCESS", "detailsUrl": ""},
                            {"__typename": "CheckRun", "name": "docs", "status": "COMPLETED", "conclusion": "SUCCESS", "detailsUrl": ""},
                            {"__typename": "CheckRun", "name": "tests", "status": "COMPLETED", "conclusion": "FAILURE",
                             "detailsUrl": "https://github.com/acme/widgets/actions/runs/77/job/5"}
                        ]
                    }}
                }}}}})
            } else {
                return None;
            };
            Some(Reply::json(200, answer))
        })
    }

    #[test]
    fn m_v_with_no_pr_shows_the_branch_s_ci() {
        let fake = branch_github();
        let server = board_server(&fake);
        let work = widgets(&server);
        let mut client = server.attach(140, 36);
        client.wait_for("the header", |s| s[2].contains("M-c Claude"));
        client.wait_text("$");
        cd(&server, &mut client, &work);

        client.send(b"\x1bv");
        client.wait_text("✕ CI failing");
        client.wait_text("Failing: 1 of 3.");
        client.wait_text(
            "CI on feature-x at 0123456 · this tab's commit isn't the one GitHub checked",
        );
        client.wait_text("✕ tests");
        client.type_text("m");
        client.wait_text("› See the failing checks");
        client.send(b"\r");
        client.wait_text("Failing checks of");
        client.wait_text("FAIL login::token");
    }

    /// A stand-in for GitHub whose PR on feature-x conflicts with main.
    fn conflicting_github() -> Fake {
        Fake::github("test-token", |request| {
            if request.path() != "/graphql" {
                return None;
            }
            let asked = request.text();
            let pr = json!({
                "number": 12, "url": "https://github.com/acme/widgets/pull/12",
                "title": "Fix the login", "state": "OPEN", "isDraft": false,
                "baseRefName": "main", "headRefName": "feature-x",
                "headRepositoryOwner": {"login": "acme"}, "author": {"login": "jo"},
                "commits": {"totalCount": 2}, "changedFiles": 0, "body": "Why.",
                "mergeable": "CONFLICTING", "mergeStateStatus": "DIRTY",
                "reviewDecision": "REVIEW_REQUIRED", "mergeQueueEntry": null
            });
            let answer = if asked.contains("pullRequests") {
                json!({"data": {"repository": {"pullRequests": {"nodes": [pr]}}}})
            } else {
                json!({"data": {"repository": {"pullRequest": pr}}})
            };
            Some(Reply::json(200, answer))
        })
    }

    #[test]
    fn m_v_says_a_pr_has_conflicts_and_offers_resolve_conflicts() {
        let fake = conflicting_github();
        let server = board_server(&fake);
        let work = widgets(&server);
        let mut client = server.attach(140, 36);
        client.wait_for("the header", |s| s[2].contains("M-c Claude"));
        client.wait_text("$");
        cd(&server, &mut client, &work);
        let pid = panes(&server)[0]["pid"].as_u64().unwrap();

        client.send(b"\x1bv");
        client.wait_text("✕ Conflicts with main");
        client.wait_text("Not ready: it has merge conflicts.  review required");
        client.type_text("m");
        client.wait_text("› Type /resolve-conflicts into this tab's Claude");
        // No Claude runs here, which the workspace says.
        client.send(b"\r");
        client.wait_text("No Claude in this tab to resolve them.");
        client.type_text("x");
        client.wait_for("the message closed", |s| {
            !s.iter().any(|l| l.contains("No Claude in this tab"))
        });

        // With Claude in the pane the view stands in for, it comes back
        // with the command typed into it.
        let view = panes(&server);
        assert_eq!(
            view[0]["options"]["@pr_view"], "1",
            "in Claude's place: {view:?}"
        );
        let sessions = server.home.join(".claude/sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join(format!("{pid}.json")),
            json!({"pid": pid, "entrypoint": "cli", "sessionId": "s-1"}).to_string(),
        )
        .unwrap();
        client.type_text("m");
        client.wait_text("› Type /resolve-conflicts into this tab's Claude");
        client.send(b"\r");
        client.wait_for("Claude back with the command typed", |s| {
            s.iter().any(|l| l.contains("$ /resolve-conflicts"))
        });
        assert!(panes(&server)[0]["options"].get("@pr_view").is_none());
    }

    #[test]
    fn m_v_counts_an_open_thread_past_the_first_page() {
        let fake = paged_github();
        let server = TestServer::start_with_env_and_settings(
            &[
                ("GH_TOKEN", "test-token"),
                ("SWITCHBOARD_GITHUB_GRAPHQL", &fake.graphql_url()),
                ("SWITCHBOARD_GITHUB_API", &fake.url),
            ],
            &json!({"notifications": false, "tips": false, "linear": false, "sidebar": false, "photos": false}).to_string(),
        );
        let work = widgets(&server);
        let mut client = server.attach(140, 36);
        client.wait_for("the header", |s| s[2].contains("M-c Claude"));
        client.wait_text("$");
        cd(&server, &mut client, &work);

        client.send(b"\x1bv");
        let screen = client.wait_text("REVIEW COMMENTS");
        let row = screen
            .iter()
            .find(|l| l.contains("REVIEW COMMENTS"))
            .unwrap();
        assert!(row.trim_end().ends_with("1 open"), "{row:?}");
        client.wait_text("src/late.rs:3");
    }

    #[test]
    fn the_branch_s_ticket_and_pr_are_copied_and_its_merge_queue_opened() {
        let fake = fake_github();
        let server = TestServer::start_with_env_and_settings(
            &[
                ("GH_TOKEN", "test-token"),
                ("SWITCHBOARD_GITHUB_GRAPHQL", &fake.graphql_url()),
                ("SWITCHBOARD_GITHUB_API", &fake.url),
                ("LINEAR_WORKSPACE", "acme"),
            ],
            &all_settings(json!({"sidebar": false})).to_string(),
        );
        let did = server.dir.path().join("did");
        desktop_stand_ins(&server.home, &did);
        let work = server.dir.path().join("widgets");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q"]);
        git(&work, &["symbolic-ref", "HEAD", "refs/heads/eng-42-login"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widgets.git",
            ],
        );
        let mut client = server.attach(120, 30);
        client.wait_text("$");
        client.type_text(&format!("cd {}\r", work.display()));
        let deadline = common::deadline(std::time::Duration::from_secs(10));
        while !panes(&server)[0]["cwd"]
            .as_str()
            .is_some_and(|d| d.ends_with("widgets"))
        {
            assert!(Instant::now() < deadline, "the shell didn't cd");
            std::thread::sleep(Duration::from_millis(50));
        }

        for (command, copied) in [
            ("copy-ticket-id", "Copied ENG-42"),
            (
                "copy-ticket-url",
                "Copied https://linear.app/acme/issue/ENG-42",
            ),
            ("copy-pr-number", "Copied 12"),
            (
                "copy-pr-url",
                "Copied https://github.com/acme/widgets/pull/12",
            ),
        ] {
            server.ok(&["press", command]);
            client.wait_text(copied);
            client.type_text("x");
            client.wait_for("the box closed", |s| {
                !s.iter().any(|l| l.contains("Copied"))
            });
        }

        // The queue of the PR's base, in the browser.
        server.ok(&["press", "open-merge-queue"]);
        let deadline = common::deadline(std::time::Duration::from_secs(10));
        loop {
            let opened = std::fs::read_to_string(&did).unwrap_or_default();
            if opened.contains("https://github.com/acme/widgets/queue/main") {
                break;
            }
            assert!(Instant::now() < deadline, "nothing opened it: {opened:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// M-i's ticket view, left open, follows its ticket as the Tickets lookup
/// sees it change, and r reads it again at once.
#[cfg(feature = "net")]
#[test]
fn an_open_ticket_view_follows_the_ticket_as_the_lookup_sees_it_change() {
    use crate::common::http::{Fake, Reply};
    use std::sync::{Arc, Mutex};

    // The ticket's state and when it was last changed, as Linear has them.
    let now = Arc::new(Mutex::new(("In Progress", "2026-11-17T10:00:00Z")));
    let linear = Fake::linear("test-key", {
        let now = now.clone();
        move |request| {
            let query = request.query();
            let (state, updated) = *now.lock().unwrap();
            let issue = json!({
                "identifier": "ENG-42", "title": "Fix the login",
                "url": "https://linear.app/acme/issue/ENG-42", "updatedAt": updated,
                "state": { "name": state, "type": "started" },
                "priority": 2, "priorityLabel": "High", "description": "The token went stale.",
            });
            let answer = if query.contains("assignedIssues") {
                json!({ "data": { "viewer": { "assignedIssues": { "nodes": [issue] }, "canceled": { "nodes": [] } } } })
            } else if query.contains("issue(id") {
                json!({ "data": { "issue": issue } })
            } else {
                json!({ "data": {} })
            };
            Some(Reply::json(200, answer))
        }
    });
    let server = TestServer::start_with_env_and_settings(
        &[
            ("LINEAR_API_KEY", "test-key"),
            ("SWITCHBOARD_LINEAR_API", &linear.graphql_url()),
        ],
        &json!({"notifications": false, "tips": false, "github": false, "sidebar": false,
            "photos": false, "pause_when_away": false, "poll_fast": false})
        .to_string(),
    );
    let mut client = server.attach(140, 36);
    client.wait_for("the header", |s| s[2].contains("M-c Claude"));
    client.wait_text("$");
    let tab = server.list()["sessions"][0]["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["set", "-t", &tab, "@tab_name", "ENG-42: fix the login"]);
    // The lookup knows the ticket before the view opens.
    let known = || {
        let sources = serde_json::from_str::<Value>(&server.ok(&["sources"])).unwrap();
        sources["views"]["tickets"]["items"]
            .to_string()
            .contains("ENG-42")
    };
    let deadline = common::deadline(Duration::from_secs(30));
    while !known() {
        assert!(Instant::now() < deadline, "the Tickets lookup's answer");
        std::thread::sleep(Duration::from_millis(100));
    }

    client.send(b"\x1bi");
    client.wait_text("In Progress");
    client.wait_text("r read again");

    // It moves to review, and the lookup is asked again: the view, left
    // open, shows the new state.
    *now.lock().unwrap() = ("In Review", "2026-11-17T11:00:00Z");
    server.ok(&["sources", "refresh", "tickets"]);
    client.wait_for("the new state in the open view", |s| {
        s.iter().any(|l| l.contains("In Review")) && !s.iter().any(|l| l.contains("In Progress"))
    });

    // It changes again with no lookup to see it: r reads it at once.
    *now.lock().unwrap() = ("Blocked", "2026-11-17T12:00:00Z");
    client.type_text("r");
    client.wait_text("Blocked");

    // With Linear turned off since the view opened, r asks it nothing: the
    // copy stays, and the view says it couldn't read.
    let asked = |linear: &Fake| {
        linear
            .seen()
            .iter()
            .filter(|r| r.query().contains("issue(id"))
            .count()
    };
    let before = asked(&linear);
    common::write_settings(
        &server.home,
        json!({"notifications": false, "tips": false, "github": false, "linear": false,
            "sidebar": false, "photos": false, "pause_when_away": false, "poll_fast": false}),
    );
    *now.lock().unwrap() = ("Done", "2026-11-17T13:00:00Z");
    client.type_text("r");
    client.wait_text("Couldn't read it again");
    client.wait_text("Blocked");
    assert_eq!(
        asked(&linear),
        before,
        "Linear asked nothing once it is off"
    );
}

#[test]
fn open_url_opens_a_link_in_its_sites_chrome_profile() {
    let dir = TempDir::new("sbo");
    let home = dir.path().join("home");
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let opened = dir.path().join("opened");
    // Chrome on Linux, `open` on macOS: each writes down what it was given,
    // after its name.
    stand_in::desktop(
        &bin,
        &["google-chrome", "open"],
        &opened,
        &dir.path().join("clipboard"),
    );
    // Chrome's profiles, where it keeps them on either.
    let local_state = json!({"profile": {"info_cache": {
        "Default": {"name": "Person 1"}, "Profile 2": {"name": "Work"}
    }}});
    for at in [
        ".config/google-chrome/Local State",
        "Library/Application Support/Google/Chrome/Local State",
    ] {
        let file = home.join(at);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, local_state.to_string()).unwrap();
    }
    let config = home.join(".config/switchboard");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("browser.json"),
        json!({"hosts": {"linear.app": "Work"}, "default": "Default"}).to_string(),
    )
    .unwrap();

    let open = |url: &str| -> String {
        let _ = std::fs::remove_file(&opened);
        let path = std::env::var("PATH").unwrap_or_default();
        let status = Command::new(BIN)
            .args(["open-url", url])
            .env("HOME", &home)
            .env_remove("SWITCHBOARD")
            .env_remove("SWITCHBOARD_PANE")
            .env("PATH", format!("{}:{path}", bin.display()))
            .env("SWITCHBOARD_STATE_DIR", dir.path().join("s"))
            .env_remove("SWITCHBOARD_SETTINGS")
            .stdin(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        // Chrome is left running on its own, so what it wrote comes a
        // moment after.
        let deadline = common::deadline(std::time::Duration::from_secs(5));
        loop {
            if let Ok(text) = std::fs::read_to_string(&opened)
                && text.ends_with('\n')
            {
                return text.trim().to_string();
            }
            assert!(Instant::now() < deadline, "nothing opened {url}");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let chrome = |profile: &str, url: &str| {
        if cfg!(target_os = "macos") {
            format!("open -na Google Chrome --args --profile-directory={profile} {url}")
        } else {
            format!("google-chrome --profile-directory={profile} {url}")
        }
    };
    let ticket = "https://linear.app/x/issue/AB-1";
    assert_eq!(
        open(ticket),
        chrome("Profile 2", ticket),
        "the site's profile, named as Chrome shows it"
    );
    let pr = "https://github.com/o/r/pull/12345#linear.app";
    assert_eq!(open(pr), chrome("Default", pr), "else the default profile");
    stand_in::assert_none_refused(&bin);
}

#[test]
fn a_link_a_program_writes_reaches_the_terminal_as_a_link() {
    let server = start(json!({"sidebar": false}));
    let mut client = server.attach(100, 20);
    client.wait_text("$");
    client.type_text(
        "printf '\\033]8;;https://example.com/osc\\033\\\\%s\\033]8;;\\033\\\\\\n' linked-text\r",
    );
    client.wait_for("the link's text", |s| {
        s.iter()
            .any(|l| l.ends_with("linked-text") && !l.contains('\''))
    });
    let raw = String::from_utf8_lossy(&client.raw).into_owned();
    let open = raw
        .rfind("\x1b]8;;https://example.com/osc\x1b\\")
        .expect("the link sent on as OSC 8");
    let text = raw[open..].find("linked-text").expect("its text inside") + open;
    let close = raw[text..].find("\x1b]8;;\x1b\\").expect("then closed") + text;
    assert!(open < text && text < close);
}

/// Stand-ins for what copies and opens on each desktop, each as strict as
/// the real one about how it is called and writing down what it was given
/// in `did`, so a test touches neither the clipboard nor the desktop.
fn desktop_stand_ins(home: &Path, did: &Path) {
    let tools: Vec<&str> = stand_in::OPENERS
        .iter()
        .chain(stand_in::CLIPBOARD)
        .copied()
        .collect();
    stand_in::desktop(&home.join("bin"), &tools, did, &home.join("clipboard"));
}

#[test]
fn the_link_commands_list_what_the_pane_shows_and_copy_read_or_open_it() {
    let server = start(json!({"sidebar": false}));
    let did = server.dir.path().join("did");
    desktop_stand_ins(&server.home, &did);
    let work = server.dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("shot.png"), b"png").unwrap();

    let mut client = server.attach(120, 30);
    client.wait_text("$");
    // The shell's prompt waited for, so the next line isn't typed into the
    // middle of it.
    client.type_text(&format!("cd {} && echo in''-work\r", work.display()));
    client.wait_text("in-work");
    // Nothing listens on port 9, so reading a link fails at once, reaching
    // nothing.
    client.type_text(
        "printf 'see http://127.0.0.1:9/plain, \\033]8;;http://127.0.0.1:9/docs\\033\\\\the docs\\033]8;;\\033\\\\ and shot.png\\n'\r",
    );
    client.wait_for("the printed line", |s| {
        s.iter()
            .any(|l| l.starts_with("see http") && l.ends_with("shot.png"))
    });

    // Copy: the newest first, a hyperlink with its text.
    server.ok(&["press", "copy-link"]);
    let screen = client.wait_text("the docs  http://127.0.0.1:9/docs");
    // In the picker's box, not the typed line above it, which holds the
    // same links.
    let row = |s: &[String], text: &str| {
        s.iter()
            .position(|l| l.contains('│') && l.contains(text))
            .unwrap()
    };
    assert!(
        row(&screen, "the docs  http") < row(&screen, "http://127.0.0.1:9/plain"),
        "newest first:\n{}",
        screen.join("\n")
    );
    client.wait_text("ctrl-o open in browser");
    client.type_text("\r");
    client.wait_text("Copied http://127.0.0.1:9/docs");
    let raw = String::from_utf8_lossy(&client.raw).into_owned();
    assert!(
        raw.contains("\x1b]52;c;aHR0cDovLzEyNy4wLjAuMTo5L2RvY3M="),
        "to the terminal's clipboard"
    );
    close_message(&mut client, "Copied");

    // Open, with Ctrl-O: read beside the pane instead.
    server.ok(&["press", "open-link"]);
    client.wait_text("ctrl-o read it beside");
    client.type_text("plain\x0f");
    let shown = wait_panes(&server, 2);
    let view = shown
        .iter()
        .find(|p| p["options"]["@link_view"] == "1")
        .expect("a link view");
    assert_eq!(view["options"]["@link_url"], "http://127.0.0.1:9/plain");

    // An image the pane names, opened in its app.
    let first = shown
        .iter()
        .find(|p| p["options"].get("@link_view").is_none())
        .unwrap();
    server.ok(&["select-pane", "-t", first["id"].as_str().unwrap()]);
    server.ok(&["press", "open-media"]);
    client.wait_text("Images and videos");
    client.wait_text("shot.png");
    client.type_text("\r");
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    loop {
        let opened = std::fs::read_to_string(&did).unwrap_or_default();
        if opened
            .lines()
            .any(|l| l.ends_with("work/shot.png") && !l.contains("copy"))
        {
            break;
        }
        assert!(Instant::now() < deadline, "nothing opened it: {opened:?}");
        std::thread::sleep(Duration::from_millis(50));
    }

    // In the notes pane, the links are the note's, since Notes shows a
    // link's text alone.
    let notes = server.home.join("Notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(notes.join(".notes.json"), r#"{"last": "Plan.md"}"#).unwrap();
    std::fs::write(
        notes.join("Plan.md"),
        "# Plan\n\n- [ ] Read [the RFC](https://rfc.example/1)\n",
    )
    .unwrap();
    let id = first["id"].as_str().unwrap();
    server.ok(&["set", "-t", id, "@notes_pane", "1"]);
    // The note the app shows, as it keeps it on its pane.
    server.ok(&["set", "-t", id, "@notes_note", "Plan.md"]);
    server.ok(&["press", "read-link"]);
    // The note's first, chosen as the list opens, the tab's other panes'
    // after it.
    let screen = client.wait_text("the RFC  https://rfc.example/1");
    assert!(
        screen
            .iter()
            .any(|l| l.contains("› the RFC  https://rfc.example/1")),
        "the note's link first:\n{}",
        screen.join("\n")
    );
}

#[test]
fn the_link_and_tab_commands_read_the_note_open_in_the_tab_too() {
    let server = start(json!({"sidebar": false}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    for _ in 0..2 {
        server.ok(&["new-tab", "-d"]);
    }
    // A note in a folder, whose pictures are named from that folder.
    let notes = server.home.join("Notes");
    std::fs::create_dir_all(notes.join("Work")).unwrap();
    std::fs::write(notes.join(".notes.json"), r#"{"last": "Work/Plan.md"}"#).unwrap();
    // Nothing listens on port 9, and nothing here reads a link anyway.
    std::fs::write(
        notes.join("Work/Plan.md"),
        "# Plan\n\n- [ ] Read [the RFC](http://127.0.0.1:9/rfc) before tab 3, see pic.png\n",
    )
    .unwrap();
    std::fs::write(notes.join("Work/pic.png"), b"png").unwrap();
    let shot = server.dir.path().join("shot.png");
    std::fs::write(&shot, b"png").unwrap();
    let first = panes(&server)[0]["id"].as_str().unwrap().to_string();
    client.type_text(&format!(
        "printf 'see http://127.0.0.1:9/pane and tab 2, {}\\n'\r",
        shot.display()
    ));
    client.wait_text("see http://127.0.0.1:9/pane and tab 2");
    let notes_pane = server.ok(&["split", "-d", "-t", &first]).trim().to_string();
    server.ok(&["set", "-t", &notes_pane, "@notes_pane", "1"]);
    server.ok(&["set", "-t", &notes_pane, "@notes_note", "Work/Plan.md"]);

    // From the other pane: its own link first, then the note's, marked.
    server.ok(&["press", "copy-link"]);
    let screen = client.wait_text("the RFC · in the note  http://127.0.0.1:9/rfc");
    assert!(
        screen.iter().any(|l| l.contains("   2/2 "))
            && screen
                .iter()
                .any(|l| l.contains("› http://127.0.0.1:9/pane")),
        "the pane's link, then the note's:\n{}",
        screen.join("\n")
    );
    client.type_text("\x1b");
    client.wait_for("the list closed", |s| {
        !s.iter().any(|l| l.contains("Copy a link"))
    });

    // The tabs: the pane's first, then the note's.
    server.ok(&["press", "go-to-mentioned-tab"]);
    client.wait_text("Go to a tab");
    let screen = client.wait_text("   2/2 ");
    assert!(
        screen.iter().any(|l| l.contains("› 2 ")),
        "the pane's tab first:\n{}",
        screen.join("\n")
    );
    client.type_text("\x1b");
    client.wait_for("the list closed", |s| {
        !s.iter().any(|l| l.contains("Go to a tab"))
    });

    // A picture the note names, from the notes folder.
    server.ok(&["press", "open-media"]);
    client.wait_text("Images and videos");
    client.wait_text("~/Notes/Work/pic.png");
    client.type_text("\x1b");
    client.wait_for("the list closed", |s| {
        !s.iter().any(|l| l.contains("Images and videos"))
    });

    // From the notes pane: the note's link first, then the other pane's.
    server.ok(&["select-pane", "-t", &notes_pane]);
    server.ok(&["press", "copy-link"]);
    let screen = client.wait_text("   2/2 ");
    assert!(
        screen
            .iter()
            .any(|l| l.contains("› the RFC  http://127.0.0.1:9/rfc")),
        "the note's link first:\n{}",
        screen.join("\n")
    );
    client.type_text("\x1b");
    client.wait_for("the list closed", |s| {
        !s.iter().any(|l| l.contains("Copy a link"))
    });

    // And the pictures the tab's other panes name.
    server.ok(&["press", "open-media"]);
    client.wait_text("Images and videos");
    client.wait_text("~/Notes/Work/pic.png");
    client.wait_for("the other pane's picture listed", |s| {
        s.iter()
            .any(|l| l.contains(&shot.display().to_string()) && !l.contains("see http"))
    });
    client.type_text("\x1b");
    client.wait_for("the list closed", |s| {
        !s.iter().any(|l| l.contains("Images and videos"))
    });

    // A note open in another tab adds nothing.
    server.ok(&["set", "-u", "-t", &notes_pane, "@notes_pane"]);
    let elsewhere = server.list()["sessions"][0]["tabs"][1]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["set", "-t", &elsewhere, "@notes_pane", "1"]);
    server.ok(&["set", "-t", &elsewhere, "@notes_note", "Work/Plan.md"]);
    server.ok(&["select-pane", "-t", &first]);
    server.ok(&["press", "go-to-mentioned-tab"]);
    client.wait_text("Went to tab 2");
    close_message(&mut client, "Went to tab 2");

    // With the setting off, the notes pane still lists its note's links,
    // and no other pane's. The server reads the settings again a moment
    // after they change, so a list that still holds both is asked again.
    server.ok(&["set", "-u", "-t", &elsewhere, "@notes_pane"]);
    server.ok(&["set", "-t", &notes_pane, "@notes_pane", "1"]);
    server.ok(&["select-pane", "-t", &notes_pane]);
    settings(
        &server,
        json!({"sidebar": false, "pane_commands_read_note": false}),
    );
    let deadline = common::deadline(Duration::from_secs(10));
    loop {
        server.ok(&["press", "copy-link"]);
        let screen = client.wait_for("the list", |s| {
            s.iter()
                .any(|l| l.contains("   1/1 ") || l.contains("   2/2 "))
        });
        if screen.iter().any(|l| l.contains("   1/1 ")) {
            assert!(
                screen
                    .iter()
                    .any(|l| l.contains("› the RFC  http://127.0.0.1:9/rfc")),
                "the note's link alone:\n{}",
                screen.join("\n")
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "still the other pane's:\n{}",
            screen.join("\n")
        );
        client.type_text("\x1b");
        client.wait_for("the list closed", |s| {
            !s.iter().any(|l| l.contains("Copy a link"))
        });
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Runs `switchboard hook <event>` in `pane` of the test's server with
/// `input` on its stdin, as Claude Code runs its hooks there.
fn hook(server: &TestServer, pane: &str, event: &str, input: Value) {
    use std::io::Write;
    let mut child = Command::new(BIN)
        .args(["hook", event])
        .env("HOME", &server.home)
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env("SWITCHBOARD", server.socket())
        .env("SWITCHBOARD_PANE", pane)
        .env_remove("SWITCHBOARD_SETTINGS")
        .env_remove("SLACK_RELAY_HELPER")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("switchboard runs");
    let _ = child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input.to_string().as_bytes());
    let out = child.wait_with_output().expect("switchboard ends");
    assert!(out.status.success(), "hook {event}: {}", server.log());
}

#[test]
fn the_link_commands_list_the_links_you_gave_the_panes_claude() {
    let server = start(json!({"sidebar": false}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    let pane = panes(&server)[0]["id"].as_str().unwrap().to_string();
    // Nothing listens on port 9, and nothing here reads a link anyway.
    let prompt = |text: &str| json!({"hook_event_name": "UserPromptSubmit", "prompt": text});

    // A link in a prompt, which Claude Code would show as "[Pasted text
    // #1]": on no screen, and listed.
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        prompt("look at this:\nhttp://127.0.0.1:9/pasted."),
    );
    server.ok(&["press", "copy-link"]);
    client.wait_text("you gave it  http://127.0.0.1:9/pasted");
    client.type_text("\r");
    client.wait_text("Copied http://127.0.0.1:9/pasted");
    close_message(&mut client, "Copied");

    // A fresh session starts with none.
    hook(&server, &pane, "SessionStart", json!({"source": "clear"}));
    server.ok(&["press", "copy-link"]);
    client.wait_text("No links in this pane");
    close_message(&mut client, "No links in this pane");

    // One the pane shows too is listed once, as the pane's.
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        prompt("http://127.0.0.1:9/pasted and http://127.0.0.1:9/shot.png"),
    );
    client.type_text("echo http://127.0.0.1:9/pasted\r");
    client.wait_for("the echoed link", |s| {
        s.iter().any(|l| l.starts_with("http://127.0.0.1:9/pasted"))
    });
    server.ok(&["press", "copy-link"]);
    let screen = client.wait_text("you gave it  http://127.0.0.1:9/shot.png");
    assert!(
        screen.iter().any(|l| l.contains("│   2/2 "))
            && !screen
                .iter()
                .any(|l| l.contains("you gave it  http://127.0.0.1:9/pasted")),
        "the pane's link once, then the prompt's:\n{}",
        screen.join("\n")
    );
    // A lone Esc is taken once nothing follows it, so the list is waited
    // out before the next command.
    client.type_text("\x1b");
    client.wait_for("the list closed", |s| {
        !s.iter().any(|l| l.contains("Copy a link"))
    });

    // A link to a picture among them is one of the pane's pictures.
    server.ok(&["press", "open-media"]);
    client.wait_text("Images and videos");
    client.wait_text("you gave it  http://127.0.0.1:9/shot.png");
}

#[test]
fn the_link_commands_list_the_newest_link_first_across_the_pane_and_prompts() {
    let server = start(json!({"sidebar": false}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    let pane = panes(&server)[0]["id"].as_str().unwrap().to_string();
    // Nothing listens on port 9, and nothing here reads a link anyway.
    client.type_text("echo http://127.0.0.1:9/screen\r");
    client.wait_for("the echoed link", |s| {
        s.iter().any(|l| l.starts_with("http://127.0.0.1:9/screen"))
    });
    let first = |client: &mut common::TestClient, row: &str| {
        // The chosen row, in the box rather than the echoed line.
        let chosen = format!("› {row}");
        client.wait_for(&chosen, |s| s.iter().any(|l| l.contains(&chosen)));
        client.type_text("\x1b");
        client.wait_for("the list closed", |s| {
            !s.iter().any(|l| l.contains("Copy a link"))
        });
    };

    // A prompt sent after the screen showed its link leads it.
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        json!({"prompt": "look at http://127.0.0.1:9/sent"}),
    );
    server.ok(&["press", "copy-link"]);
    first(&mut client, "you gave it  http://127.0.0.1:9/sent");

    // A turn that ends naming the screen's link puts it first again.
    std::thread::sleep(Duration::from_millis(5) * common::patience());
    hook(
        &server,
        &pane,
        "Stop",
        json!({"last_assistant_message": "Done: http://127.0.0.1:9/screen"}),
    );
    server.ok(&["press", "copy-link"]);
    first(&mut client, "http://127.0.0.1:9/screen");
}

#[test]
fn with_prompt_links_off_the_link_commands_list_only_what_the_pane_shows() {
    let server = start(json!({"sidebar": false, "prompt_links": false}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    let pane = panes(&server)[0]["id"].as_str().unwrap().to_string();
    // The server reads the settings again a moment after they change, so
    // a list that still shows the link is closed and asked for again.
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    loop {
        hook(
            &server,
            &pane,
            "UserPromptSubmit",
            json!({"prompt": "see http://127.0.0.1:9/pasted"}),
        );
        server.ok(&["press", "copy-link"]);
        let screen = client.wait_for("an answer", |s| {
            s.iter()
                .any(|l| l.contains("No links in this pane") || l.contains("you gave it"))
        });
        if screen.iter().any(|l| l.contains("No links in this pane")) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "still listed:\n{}",
            screen.join("\n")
        );
        client.type_text("\x1b");
        hook(&server, &pane, "SessionStart", json!({"source": "clear"}));
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// What the image views keep, under a test's home.
#[test]
fn m_m_does_nothing_now_that_m_v_answers_it() {
    let server = start(json!({"linear": false}));
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    let (code, _, said) = server.run(&["press", "pr-ready"]);
    assert_ne!(code, 0, "no action is named pr-ready: {said}");
    // M-m, then M-p's palette on the same connection, so the palette shows
    // only once M-m was handled: no box of M-m's is there with it.
    client.send(b"\x1bm");
    client.send(b"\x1bp");
    let screen = client.wait_for("the palette", |s| s.iter().any(|l| l.contains('╭')));
    assert!(
        !screen
            .iter()
            .any(|l| l.contains("Ready to merge?") || l.contains("Reaching out to GitHub")),
        "{}",
        screen.join("\n")
    );
}

fn image_cache(home: &Path) -> switchboard::views::image::cache::Cache {
    switchboard::views::image::cache::Cache::at(home.join(".cache/switchboard"))
}

#[test]
fn the_palette_clears_the_image_cache_after_a_yes() {
    let server = start(json!({"sidebar": false}));
    let cache = image_cache(&server.home);
    let picture = cache
        .keep_asset("https://example.com/a.png", "png", b"picture")
        .unwrap();
    let text = server.home.join(".cache/switchboard/pr-image/ENG-1.json");
    std::fs::create_dir_all(text.parent().unwrap()).unwrap();
    std::fs::write(&text, "{}").unwrap();
    let other = server.home.join(".cache/switchboard/tab-names.json");
    std::fs::write(&other, "{}").unwrap();

    let mut client = server.attach(100, 30);
    client.wait_text("$");
    server.ok(&["press", "clear-image-cache"]);
    client.wait_text("Clear the PR and ticket image cache? [Y/n]");
    client.type_text("n");
    client.wait_for("the question gone", |s| {
        !s.iter().any(|l| l.contains("image cache?"))
    });
    assert!(picture.exists() && text.exists(), "no is no");

    server.ok(&["press", "clear-image-cache"]);
    client.wait_text("Clear the PR and ticket image cache? [Y/n]");
    client.type_text("\r");
    client.wait_text("The PR and ticket image cache is cleared.");
    assert!(!cache.assets().exists(), "the copies gone");
    assert!(!text.exists(), "and the text last read");
    assert!(other.exists(), "nothing else of the cache's");
}

/// `switchboard pr-image --ticket ENG-1` with `PR_IMAGE_DUMP`, in a home of
/// the test's own, with no terminal (so its pane is 100 by 50 cells of 16
/// by 32 pixels) and nothing on its PATH, Chrome included; Linear moved to
/// a port nothing listens on.
#[cfg(target_os = "linux")]
fn dump_ticket(home: &Path, out: &Path) -> std::process::Output {
    use std::os::unix::process::CommandExt;
    let bin = home.join("empty-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let mut cmd = Command::new(BIN);
    cmd.args(["pr-image", "--ticket", "ENG-1"])
        .env_clear()
        .env("HOME", home)
        .env("PATH", &bin)
        .env("PR_IMAGE_DUMP", out)
        .env("SWITCHBOARD_LINEAR_API", "http://127.0.0.1:9")
        .stdin(Stdio::null());
    // SAFETY: setsid alone, between fork and exec, so the program has no
    // terminal to ask its size of.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.output().expect("pr-image runs")
}

/// A ticket opened again at the same size shows the pages kept, without
/// looking for Chrome (Linux alone, where the PATH is where Chrome is
/// looked for, so a failure can't start a real one).
#[cfg(target_os = "linux")]
#[test]
fn a_ticket_drawn_before_shows_from_the_cache_without_chrome() {
    use switchboard::views::image::page::Parts;
    use switchboard::views::image::render::{Shot, View};
    let dir = TempDir::new("sb-image-cache");
    let home = dir.path().join("home");
    let parts = Parts {
        title: "Fix the login".into(),
        number: "ENG-1".into(),
        html: "<p>hi</p>".into(),
        ..Parts::default()
    };
    let text = home.join(".cache/switchboard/pr-image/ENG-1.json");
    std::fs::create_dir_all(text.parent().unwrap()).unwrap();
    std::fs::write(&text, serde_json::to_string(&parts).unwrap()).unwrap();
    let drawn = dir.path().join("page-1-1.png");
    std::fs::write(&drawn, "page one").unwrap();
    let view = View::new(100, 50, (16, 32));
    let shot = Shot {
        file: drawn,
        rows: 10,
        from: 0.0,
    };
    image_cache(&home).save_pages(&parts, &view, &[shot]);

    let out = dir.path().join("dump");
    let run = dump_ticket(&home, &out);
    assert!(run.status.success(), "{run:?}");
    assert_eq!(
        std::fs::read_to_string(out.join("page-1.png")).unwrap(),
        "page one"
    );

    // With the cache turned off, it has to be drawn, and there is no
    // Chrome to draw it.
    let config = home.join(".config/switchboard");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("settings.json"), r#"{"image_cache": false}"#).unwrap();
    let run = dump_ticket(&home, &dir.path().join("dump-off"));
    assert!(!run.status.success());
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("needs Google Chrome or Chromium"),
        "{run:?}"
    );
}

#[test]
fn opening_a_link_says_where_it_is_going_until_the_browser_shows_it() {
    let server = start(json!({}));
    let did = server.dir.path().join("did");
    let tools: Vec<&str> = stand_in::OPENERS
        .iter()
        .chain(stand_in::CLIPBOARD)
        .chain(stand_in::CHROMES)
        .copied()
        .collect();
    stand_in::desktop(
        &server.home.join("bin"),
        &tools,
        &did,
        &server.home.join("clipboard"),
    );
    let local_state = json!({"profile": {"info_cache": {
        "Default": {"name": "Person 1"}, "Profile 2": {"name": "Work"}
    }}});
    for at in [
        ".config/google-chrome/Local State",
        "Library/Application Support/Google/Chrome/Local State",
    ] {
        let file = server.home.join(at);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, local_state.to_string()).unwrap();
    }
    let config = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("browser.json"),
        json!({"hosts": {"github.com": "Profile 2"}}).to_string(),
    )
    .unwrap();

    let mut client = server.attach(120, 30);
    client.wait_text("$");
    client.type_text("echo https://github.com/o/r/pull/12345\r");
    client.wait_for("the printed link", |s| {
        s.iter()
            .any(|l| l.ends_with("│https://github.com/o/r/pull/12345"))
    });
    let opens = || {
        std::fs::read_to_string(&did)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("--profile-directory=Profile 2"))
            .count()
    };
    let wait_opens = |n: usize| {
        let deadline = common::deadline(Duration::from_secs(10));
        while opens() < n {
            assert!(Instant::now() < deadline, "the link wasn't opened");
            std::thread::sleep(Duration::from_millis(50));
        }
    };

    server.ok(&["press", "open-link"]);
    client.wait_text("ctrl-o read it beside");
    client.type_text("\r");
    client.wait_text("Opening github.com in Chrome · Work…");
    wait_opens(1);
    close_message(&mut client, "Opening github.com");

    // Off, it opens with nothing said. The sidebar going too shows the
    // settings were read again.
    settings(
        &server,
        json!({"sidebar": false, "link_opening_message": false}),
    );
    let deadline = common::deadline(Duration::from_secs(10));
    while server.list()["sessions"][0]["sidebar"]["columns"] != 0 {
        assert!(Instant::now() < deadline, "the settings weren't read");
        std::thread::sleep(Duration::from_millis(50));
    }
    server.ok(&["press", "open-link"]);
    client.wait_text("ctrl-o read it beside");
    client.type_text("\r");
    wait_opens(2);
    client.wait_for("the picker closed", |s| {
        !s.iter().any(|l| l.contains("ctrl-o read it beside"))
    });
    std::thread::sleep(Duration::from_millis(300) * common::patience());
    assert!(
        !client
            .screen()
            .iter()
            .any(|l| l.contains("Opening github.com")),
        "nothing said with the setting off:\n{}",
        client.screen().join("\n")
    );
    stand_in::assert_none_refused(&server.home.join("bin"));
}

/// The current tab's number in the first session.
fn current_number(server: &TestServer) -> i64 {
    server.list()["sessions"][0]["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["current"] == true)
        .unwrap()["index"]
        .as_i64()
        .unwrap()
}

#[test]
fn the_tab_commands_go_to_or_copy_the_open_tabs_a_pane_names() {
    let server = start(json!({"sidebar": false}));
    let did = server.dir.path().join("did");
    desktop_stand_ins(&server.home, &did);
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    for _ in 0..3 {
        server.ok(&["new-tab", "-d"]);
    }

    // Tab 9 isn't open, so tab 3 is the one named: gone to at once.
    client.type_text("printf 'see tab 3 and tab 9\\n'\r");
    client.wait_text("see tab 3 and tab 9");
    server.ok(&["press", "go-to-mentioned-tab"]);
    client.wait_text("Went to tab 3");
    client.wait_for("tab 3 in front", |_| current_number(&server) == 3);
    close_message(&mut client, "Went to tab 3");

    client.wait_text("$");
    client.type_text("printf 'tab 2 and Tab #4\\n'\r");
    client.wait_text("tab 2 and Tab #4");

    // Copy: the newest first, Enter copying its number.
    server.ok(&["press", "copy-mentioned-tab"]);
    client.wait_text("Copy a tab's number");
    client.wait_text("ctrl-o go to it");
    client.type_text("\r");
    client.wait_text("Copied tab 4");
    let raw = String::from_utf8_lossy(&client.raw).into_owned();
    assert!(
        raw.contains("\x1b]52;c;NA=="),
        "to the terminal's clipboard"
    );
    close_message(&mut client, "Copied tab 4");

    // Go: both listed, the second picked.
    server.ok(&["press", "go-to-mentioned-tab"]);
    client.wait_text("Go to a tab");
    client.type_text("\x1b[B\r");
    client.wait_for("tab 2 in front", |_| current_number(&server) == 2);

    // Tab 2's pane names none.
    server.ok(&["press", "go-to-mentioned-tab"]);
    client.wait_text("No tabs named in this pane");
}

/// M-i opens a tab's ticket from where its work's tickets are kept: the
/// GitHub issue its branch was started for, a project a name you gave it
/// names, or, with none, one of yours you choose, kept on the tab.
#[cfg(feature = "net")]
mod tickets {
    use super::*;
    use crate::common::http::{Fake, Reply};

    /// repos.json giving acme/widgets' `tickets` as `entry` says.
    fn repos(server: &TestServer, entry: Value) {
        let config = server.home.join(".config/switchboard");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("repos.json"),
            json!({ "acme/widgets": entry }).to_string(),
        )
        .unwrap();
    }

    /// The first tab's id and its directory set to `dir` by a `cd`.
    fn cd(server: &TestServer, client: &mut common::TestClient, dir: &Path) -> String {
        client.type_text(&format!("cd {}\r", dir.display()));
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let deadline = common::deadline(Duration::from_secs(10));
        while !panes(server)[0]["cwd"]
            .as_str()
            .is_some_and(|d| d.ends_with(&name))
        {
            assert!(Instant::now() < deadline, "the shell didn't cd");
            std::thread::sleep(Duration::from_millis(50));
        }
        server.list()["sessions"][0]["tabs"][0]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn m_i_on_an_issue_branch_opens_the_issue_with_its_body_and_comments() {
        let github = Fake::github("test-token", |request| {
            let path = request.path();
            if path.starts_with("/repos/acme/widgets/issues/12/comments") {
                Some(Reply::json(
                    200,
                    json!([{ "body": "Which widget?", "created_at": "2026-10-02T00:00:00Z",
                        "user": { "login": "jo" } }]),
                ))
            } else if path.starts_with("/repos/acme/widgets/issues/12") {
                Some(Reply::json(
                    200,
                    json!({ "number": 12, "title": "Make the widget blue", "state": "open",
                        "body": "It is red.", "user": { "login": "sam" },
                        "created_at": "2026-10-01T09:00:00Z", "updated_at": "2026-10-01T09:00:00Z",
                        "assignees": [], "labels": [], "comments": 1 }),
                ))
            } else {
                None
            }
        });
        // Linear off from the start, so M-i offers no choice of its tickets.
        let server = TestServer::start_with_env_and_settings(
            &[
                ("GH_TOKEN", "test-token"),
                ("SWITCHBOARD_GITHUB_GRAPHQL", &github.graphql_url()),
                ("SWITCHBOARD_GITHUB_API", &github.url),
            ],
            &json!({"notifications": false, "tips": false, "linear": false}).to_string(),
        );
        repos(&server, json!({ "landing": "pr" }));
        let work = server.dir.path().join("widgets");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q"]);
        git(
            &work,
            &["symbolic-ref", "HEAD", "refs/heads/issue/12-make-it-blue"],
        );
        git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widgets.git",
            ],
        );
        let mut client = server.attach(140, 36);
        client.wait_text("$");
        cd(&server, &mut client, &work);

        // With the repository's tickets not set, nothing guesses: no
        // ticket for the issue's branch, and M-i says how to set them.
        client.send(b"\x1bi");
        client.wait_text("acme/widgets's ticket source isn't set");
        assert_eq!(panes(&server).len(), 1, "no ticket view opened");
        close_message(&mut client, "acme/widgets's ticket source isn't set");
        repos(&server, json!({ "tickets": "github" }));

        // Linear is off, and the issue still opens: it is GitHub's.
        client.send(b"\x1bi");
        client.wait_text("#12  Make the widget blue");
        client.wait_text("It is red.");
        client.wait_text("Which widget?");
        // In the pane's place.
        let shown = wait_panes(&server, 1);
        assert_eq!(
            shown[0]["options"]["@ticket_view_id"], "acme/widgets#12",
            "{shown:?}"
        );
    }

    #[test]
    fn m_v_takes_the_ticket_s_place_beside_a_narrow_lone_pane() {
        let github = Fake::github("test-token", |request| {
            let path = request.path();
            if path.starts_with("/repos/acme/widgets/issues/12/comments") {
                return Some(Reply::json(200, json!([])));
            }
            if path.starts_with("/repos/acme/widgets/issues/12") {
                return Some(Reply::json(
                    200,
                    json!({ "number": 12, "title": "Make the widget blue", "state": "open",
                        "body": "It is red.", "user": { "login": "sam" },
                        "created_at": "2026-10-01T09:00:00Z", "updated_at": "2026-10-01T09:00:00Z",
                        "assignees": [], "labels": [], "comments": 0 }),
                ));
            }
            if path != "/graphql" {
                return None;
            }
            let pr = json!({
                "number": 41, "url": "https://github.com/acme/widgets/pull/41",
                "title": "Blue widgets", "state": "OPEN", "isDraft": false,
                "baseRefName": "main", "headRefName": "issue/12-make-it-blue",
                "headRepositoryOwner": {"login": "acme"}, "author": {"login": "jo"},
                "commits": {"totalCount": 1}, "changedFiles": 0, "body": "Why.",
                "mergeable": "CONFLICTING", "mergeStateStatus": "DIRTY", "mergeQueueEntry": null
            });
            let answer = if request.text().contains("pullRequests") {
                json!({"data": {"repository": {"pullRequests": {"nodes": [pr]}}}})
            } else {
                json!({"data": {"repository": {"pullRequest": pr}}})
            };
            Some(Reply::json(200, answer))
        });
        let server = TestServer::start_with_env_and_settings(
            &[
                ("GH_TOKEN", "test-token"),
                ("SWITCHBOARD_GITHUB_GRAPHQL", &github.graphql_url()),
                ("SWITCHBOARD_GITHUB_API", &github.url),
            ],
            &json!({"notifications": false, "tips": false, "linear": false,
                "sidebar": false, "photos": false, "lone_pane_width": "80"})
            .to_string(),
        );
        repos(&server, json!({ "tickets": "github" }));
        let work = server.dir.path().join("widgets");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q"]);
        git(
            &work,
            &["symbolic-ref", "HEAD", "refs/heads/issue/12-make-it-blue"],
        );
        git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widgets.git",
            ],
        );
        let mut client = server.attach(240, 36);
        client.wait_text("$");
        cd(&server, &mut client, &work);
        let first = panes(&server)[0]["id"].as_str().unwrap().to_string();
        let wait_until = |what: &str, done: &dyn Fn(&[Value]) -> bool| {
            let deadline = common::deadline(Duration::from_secs(10));
            loop {
                let now = panes(&server);
                if done(&now) {
                    return now;
                }
                assert!(Instant::now() < deadline, "waiting for {what}: {now:?}");
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        wait_until("the pane capped", &|p| p[0]["cols"] == 80);

        // The ticket beside the pane, in its spare columns.
        client.send(b"\x1bi");
        client.wait_text("It is red.");
        let shown = wait_until("the ticket beside", &|p| p.len() == 2);
        assert_eq!(shown[0]["cols"], 80, "{shown:?}");
        let spare = shown[1]["cols"].clone();

        // M-v: the PR in its place, the pane as wide as it was.
        client.send(b"\x1bv");
        client.wait_text("✕ Conflicts with main");
        let shown = wait_until("the PR beside", &|p| {
            p.len() == 2 && p[1]["options"]["@pr_view"] == "1"
        });
        assert_eq!(shown[0]["id"], first.as_str(), "{shown:?}");
        assert_eq!(shown[0]["cols"], 80, "{shown:?}");
        assert_eq!(shown[1]["cols"], spare, "{shown:?}");
        assert!(
            !shown
                .iter()
                .any(|p| p["options"].get("@ticket_view").is_some()),
            "the ticket closed: {shown:?}"
        );
        // Its body, since the tab's header names the issue too.
        client.wait_for("the ticket gone", |s| {
            !s.iter().any(|l| l.contains("It is red."))
        });

        // M-v again: the columns left empty, the ticket not brought back.
        client.send(b"\x1bv");
        let shown = wait_until("one pane", &|p| p.len() == 1);
        assert_eq!(shown[0]["id"], first.as_str());
        assert_eq!(shown[0]["cols"], 80);
    }

    /// A tab on an issue's branch is named by the issue's number and title,
    /// its M-T box names the issue, and the copy commands copy `#12` and
    /// its link.
    #[test]
    fn a_tab_on_an_issue_is_named_by_it_and_copies_its_number_and_link() {
        let github = Fake::github("test-token", |request| {
            request
                .path()
                .starts_with("/repos/acme/widgets/issues/12")
                .then(|| {
                    Reply::json(
                        200,
                        json!({ "number": 12, "title": "Make the widget blue", "state": "open" }),
                    )
                })
        });
        let server = TestServer::start_with_env_and_settings(
            &[
                ("GH_TOKEN", "test-token"),
                ("SWITCHBOARD_GITHUB_GRAPHQL", &github.graphql_url()),
                ("SWITCHBOARD_GITHUB_API", &github.url),
            ],
            &all_settings(json!({"linear": false})).to_string(),
        );
        repos(&server, json!({ "tickets": "github" }));
        let work = server.dir.path().join("widgets");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q"]);
        git(
            &work,
            &["symbolic-ref", "HEAD", "refs/heads/issue/12-make-it-blue"],
        );
        git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widgets.git",
            ],
        );
        let mut client = server.attach(140, 36);
        client.wait_text("$");
        cd(&server, &mut client, &work);

        let label = || {
            server.list()["sessions"][0]["tabs"][0]["label"]
                .as_str()
                .unwrap_or("")
                .to_string()
        };
        let deadline = common::deadline(Duration::from_secs(20));
        while !label().ends_with("#12: Make the widget blue") {
            assert!(Instant::now() < deadline, "the tab is {:?}", label());
            std::thread::sleep(Duration::from_millis(100));
        }

        server.ok(&["press", "tab-title"]);
        client.wait_text("Ticket     #12");
        client.send(b"\x1b");
        client.wait_for("the box closed", |s| {
            !s.iter().any(|l| l.contains("Ticket     #12"))
        });

        server.ok(&["press", "copy-ticket-id"]);
        client.wait_text("Copied #12");
        close_message(&mut client, "Copied #12");
        server.ok(&["press", "copy-ticket-url"]);
        client.wait_text("Copied https://github.com/acme/widgets/issues/12");
    }

    /// Linear, answering your tickets and projects, a ticket and a project.
    fn fake_linear() -> Fake {
        Fake::linear("test-key", |request| {
            let query = request.query();
            let answer = if query.contains("assignedIssues(first: 50") {
                json!({ "data": { "viewer": { "assignedIssues": { "nodes": [
                    { "identifier": "AB-1", "title": "Old work", "createdAt": "2026-01-01T00:00:00Z", "state": { "name": "Todo" } },
                    { "identifier": "AB-2", "title": "New work", "createdAt": "2026-03-01T00:00:00Z", "state": { "name": "Todo" } }
                ] } } } })
            } else if query.contains("projects(first: 50") {
                json!({ "data": { "projects": { "nodes": [] } } })
            } else if query.contains("project(id") {
                json!({ "data": { "project": { "name": "Review cost", "description": "Cut what a round costs.",
                    "status": { "name": "Backlog", "type": "backlog" }, "issues": { "nodes": [] } } } })
            } else if query.contains("issue(id") {
                json!({ "data": { "issue": { "identifier": "AB-2", "title": "New work",
                    "description": "The newest.", "labels": { "nodes": [] } } } })
            } else {
                json!({ "data": {} })
            };
            Some(Reply::json(200, answer))
        })
    }

    fn linear_server(linear: &Fake) -> TestServer {
        TestServer::start_with_env_and_settings(
            &[
                ("LINEAR_API_KEY", "test-key"),
                ("SWITCHBOARD_LINEAR_API", &linear.graphql_url()),
            ],
            &json!({"notifications": false, "tips": false, "github": false,
                "sidebar": false, "photos": false})
            .to_string(),
        )
    }

    #[test]
    fn m_i_with_no_ticket_asks_for_one_of_yours_newest_first_and_keeps_it() {
        let linear = fake_linear();
        let server = linear_server(&linear);
        let mut client = server.attach(140, 36);
        client.wait_text("$");
        let tab = server.list()["sessions"][0]["tabs"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();

        client.send(b"\x1bi");
        let screen = client.wait_text("'s ticket or project");
        let at = |id: &str| screen.iter().position(|l| l.contains(id));
        let (new, old) = (at("AB-2  New work"), at("AB-1  Old work"));
        assert!(new.is_some() && new < old, "newest first: {screen:?}");
        client.type_text("\r");
        client.wait_text("The newest.");
        let tabs = server.list()["sessions"][0]["tabs"].clone();
        assert_eq!(tabs[0]["options"]["@tab_ticket"], "AB-2", "{tabs}");

        // Kept: M-i again, after closing it, opens it without asking.
        client.send(b"\x1bi");
        wait_panes(&server, 1);
        client.send(b"\x1bi");
        client.wait_text("The newest.");
        assert_eq!(server.list()["sessions"][0]["tabs"][0]["id"], tab.as_str());
    }

    #[test]
    fn m_i_in_a_tab_named_for_a_project_opens_the_project_not_a_ticket() {
        let linear = fake_linear();
        let server = linear_server(&linear);
        let mut client = server.attach(140, 36);
        client.wait_text("$");
        let tab = server.list()["sessions"][0]["tabs"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        server.ok(&[
            "set",
            "-t",
            &tab,
            "@tab_name",
            "Shaping P-AB-4: review cost",
        ]);
        client.send(b"\x1bi");
        client.wait_text("Review cost");
        client.wait_text("Cut what a round costs.");
        assert!(
            !linear.seen().iter().any(|r| r.query().contains("issue(id")),
            "no ticket AB-4 asked for"
        );
    }

    /// A clone of acme/widgets on `main`, with no ticket in its branch,
    /// whose `tickets` in repos.json is `provider`.
    fn widgets_set_to(server: &TestServer, provider: &str) -> std::path::PathBuf {
        let work = server.dir.path().join("widgets");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q"]);
        git(&work, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widgets.git",
            ],
        );
        let config = server.home.join(".config/switchboard");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("repos.json"),
            json!({ "acme/widgets": { "tickets": provider } }).to_string(),
        )
        .unwrap();
        work
    }

    #[test]
    fn m_i_with_no_ticket_in_a_github_repository_asks_for_one_of_its_open_issues() {
        let github = Fake::github("test-token", |request| {
            let path = request.path();
            if request.query().contains("issues(states: OPEN") {
                Some(Reply::json(
                    200,
                    json!({ "data": { "viewer": { "login": "me" }, "repository": { "issues": { "nodes": [
                        { "number": 13, "title": "Someone else's", "assignees": { "nodes": [] } },
                        { "number": 12, "title": "Make the widget blue", "assignees": { "nodes": [{ "login": "me" }] } }
                    ] } } } }),
                ))
            } else if path.starts_with("/repos/acme/widgets/issues/12/comments") {
                Some(Reply::json(200, json!([])))
            } else if path.starts_with("/repos/acme/widgets/issues/12") {
                Some(Reply::json(
                    200,
                    json!({ "number": 12, "title": "Make the widget blue", "state": "open",
                        "body": "It is red.", "user": { "login": "sam" },
                        "created_at": "2026-10-01T09:00:00Z", "updated_at": "2026-10-01T09:00:00Z",
                        "assignees": [], "labels": [], "comments": 0 }),
                ))
            } else {
                None
            }
        });
        let linear = fake_linear();
        let server = TestServer::start_with_env_and_settings(
            &[
                ("GH_TOKEN", "test-token"),
                ("SWITCHBOARD_GITHUB_GRAPHQL", &github.graphql_url()),
                ("SWITCHBOARD_GITHUB_API", &github.url),
                ("LINEAR_API_KEY", "test-key"),
                ("SWITCHBOARD_LINEAR_API", &linear.graphql_url()),
            ],
            &json!({"notifications": false, "tips": false,
                "sidebar": false, "photos": false})
            .to_string(),
        );
        let work = widgets_set_to(&server, "github");
        let mut client = server.attach(140, 36);
        client.wait_text("$");
        cd(&server, &mut client, &work);

        client.send(b"\x1bi");
        let screen = client.wait_text("'s issue");
        let at = |row: &str| screen.iter().position(|l| l.contains(row));
        let (yours, other) = (
            at("#12  Make the widget blue · assigned to you"),
            at("#13  Someone else's"),
        );
        assert!(yours.is_some() && yours < other, "yours first: {screen:?}");
        assert!(
            !screen.iter().any(|l| l.contains("AB-2")),
            "no Linear ticket: {screen:?}"
        );
        client.type_text("\r");
        client.wait_text("It is red.");
        let tabs = server.list()["sessions"][0]["tabs"].clone();
        assert_eq!(
            tabs[0]["options"]["@tab_ticket"], "acme/widgets#12",
            "{tabs}"
        );
    }

    #[test]
    fn m_i_with_no_ticket_in_a_linear_repository_asks_for_one_of_your_linear_tickets() {
        let linear = fake_linear();
        let server = linear_server(&linear);
        let work = widgets_set_to(&server, "linear");
        let mut client = server.attach(140, 36);
        client.wait_text("$");
        cd(&server, &mut client, &work);

        client.send(b"\x1bi");
        client.wait_text("'s ticket or project");
        client.wait_text("AB-2  New work");
    }
}
