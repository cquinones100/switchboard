//! GitHub and Linear as the rest of Switchboard reaches them, against a small
//! HTTP server of the test's own on 127.0.0.1 (each service's address moved
//! there by its `SWITCHBOARD_*` variable): what is sent (the token, the
//! query), what is made of each kind of answer, and that every request is
//! counted in the request audit, which lands in a temporary HOME.
//!
//! The environment is the process's, so every test holds `ENV` for its whole
//! run.

#![cfg(feature = "net")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use serde_json::{Value, json};
use switchboard::integrations::{github, linear};
use switchboard::net::{self, ErrorKind};
use switchboard::{requests, settings, util};

static ENV: Mutex<()> = Mutex::new(());

/// A request as the fake server saw it.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Value,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A canned answer: status, extra headers, body.
struct Canned {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
}

fn answer(status: u16, body: Value) -> Canned {
    Canned {
        status,
        headers: Vec::new(),
        body: body.to_string(),
    }
}

/// A server answering each connection with the next canned answer, then
/// closing it, so every request comes on a connection of its own.
struct Fake {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Fake {
    fn start(answers: Vec<Canned>) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        thread::spawn(move || {
            for canned in answers {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream);
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    return;
                }
                let mut parts = first.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("").to_string();
                let mut headers = Vec::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some((n, v)) = line.split_once(':') {
                        headers.push((n.trim().to_ascii_lowercase(), v.trim().to_string()));
                    }
                }
                let length = headers
                    .iter()
                    .find(|(n, _)| n == "content-length")
                    .and_then(|(_, v)| v.parse::<usize>().ok())
                    .unwrap_or(0);
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                log.lock().unwrap().push(Seen {
                    method,
                    path,
                    headers,
                    body: serde_json::from_slice(&body).unwrap_or(Value::Null),
                });
                let mut reply = format!(
                    "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    canned.status,
                    canned.body.len()
                );
                for (n, v) in &canned.headers {
                    reply.push_str(&format!("{n}: {v}\r\n"));
                }
                reply.push_str("\r\n");
                reply.push_str(&canned.body);
                let mut stream = reader.into_inner();
                let _ = stream.write_all(reply.as_bytes());
                let _ = stream.flush();
            }
        });
        Fake { url, seen }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

/// A temporary HOME, a PATH holding only git (so no gh answers with a real
/// token), no credentials in the environment and no desktop keyring to ask;
/// the settings file in it. Holds `ENV` while it lives.
struct Env {
    _lock: MutexGuard<'static, ()>,
    home: PathBuf,
}

fn set(var: &str, value: impl AsRef<std::ffi::OsStr>) {
    // SAFETY: every test holds ENV while it changes or reads the environment.
    unsafe { std::env::set_var(var, value) }
}

fn unset(var: &str) {
    // SAFETY: as in `set`.
    unsafe { std::env::remove_var(var) }
}

impl Env {
    fn new(name: &str) -> Env {
        let lock = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let home = std::env::temp_dir().join(format!(
            "sb-int-{name}-{}-{}",
            std::process::id(),
            util::now_ms()
        ));
        std::fs::create_dir_all(home.join("bin")).unwrap();
        static GIT: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
        if let Some(git) = GIT.get_or_init(|| util::which("git")) {
            std::os::unix::fs::symlink(git, home.join("bin/git")).unwrap();
        }
        set("HOME", &home);
        set("PATH", home.join("bin"));
        set("SWITCHBOARD_SETTINGS", home.join("settings.json"));
        for var in [
            "SWITCHBOARD_STATE_DIR",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "LINEAR_API_KEY",
            "DBUS_SESSION_BUS_ADDRESS",
            "XDG_RUNTIME_DIR",
            "SWITCHBOARD_GITHUB_API",
            "SWITCHBOARD_GITHUB_GRAPHQL",
            "SWITCHBOARD_LINEAR_API",
        ] {
            unset(var);
        }
        settings::reload_settings();
        github::forget_token();
        linear::forget_key();
        Env { _lock: lock, home }
    }

    fn settings(&self, value: Value) {
        std::fs::write(self.home.join("settings.json"), value.to_string()).unwrap();
        settings::reload_settings();
    }

    fn git(&self, dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git runs");
        assert!(status.status.success(), "git {args:?}: {status:?}");
    }

    /// A repository at `name` under HOME, on `branch` (unborn), with origin.
    fn repo(&self, name: &str, origin: &str, branch: &str) -> PathBuf {
        let dir = self.home.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        self.git(&dir, &["init", "-q"]);
        self.git(
            &dir,
            &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
        );
        self.git(&dir, &["remote", "add", "origin", origin]);
        dir
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn kind(err: &anyhow::Error) -> ErrorKind {
    net::kind_of(err)
        .cloned()
        .unwrap_or_else(|| panic!("not a service error: {err:#}"))
}

/// Today's count for a service and protocol.
fn counted(service: &str, protocol: &str) -> usize {
    requests::tally()
        .services
        .iter()
        .find(|s| s.service == service)
        .and_then(|s| s.protocols.get(protocol).copied())
        .unwrap_or(0)
}

#[test]
fn github_graphql_sends_the_token_and_query_and_is_counted() {
    let env = Env::new("gh-graphql");
    set("GH_TOKEN", "tok-1");
    let fake = Fake::start(vec![answer(
        200,
        json!({ "data": { "viewer": { "login": "me" } } }),
    )]);
    set(
        "SWITCHBOARD_GITHUB_GRAPHQL",
        format!("{}/graphql", fake.url),
    );
    let data = util::block_on(github::graphql(
        "query { viewer { login } }",
        json!({ "x": 1 }),
        "test: viewer",
    ))
    .unwrap();
    assert_eq!(data["viewer"]["login"], "me");
    let seen = fake.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].path, "/graphql");
    assert_eq!(seen[0].header("authorization"), Some("Bearer tok-1"));
    assert!(
        seen[0]
            .header("user-agent")
            .is_some_and(|ua| ua.starts_with("Switchboard/"))
    );
    assert_eq!(seen[0].body["query"], "query { viewer { login } }");
    assert_eq!(seen[0].body["variables"], json!({ "x": 1 }));
    assert_eq!(counted("GitHub", "GraphQL"), 1);
    let day = util::ymd(&util::local_now());
    let log = std::fs::read_to_string(
        env.home
            .join(format!(".local/state/switchboard/requests/{day}.tsv")),
    )
    .unwrap();
    assert!(log.contains("\tGitHub\tGraphQL\ttest: viewer\n"), "{log}");
}

#[test]
fn github_rest_asks_for_its_own_json() {
    let _env = Env::new("gh-rest");
    set("GITHUB_TOKEN", "tok-2");
    let fake = Fake::start(vec![answer(200, json!({ "default_branch": "trunk" }))]);
    set("SWITCHBOARD_GITHUB_API", &fake.url);
    let body = util::block_on(github::rest("GET", "/repos/acme/widgets", None, "test")).unwrap();
    assert_eq!(body["default_branch"], "trunk");
    let seen = fake.seen();
    assert_eq!(seen[0].method, "GET");
    assert_eq!(seen[0].path, "/repos/acme/widgets");
    assert_eq!(
        seen[0].header("accept"),
        Some("application/vnd.github+json")
    );
    assert_eq!(seen[0].header("authorization"), Some("Bearer tok-2"));
    assert_eq!(counted("GitHub", "REST"), 1);
}

#[test]
fn github_failures_say_what_kind() {
    let _env = Env::new("gh-errors");
    set("GH_TOKEN", "tok-3");
    let reset = util::now_secs() + 600;
    let fake = Fake::start(vec![
        answer(
            200,
            json!({ "data": { "repository": null }, "errors": [{ "type": "NOT_FOUND",
                "message": "Could not resolve to a Repository with the name 'acme/nope'." }] }),
        ),
        Canned {
            status: 200,
            headers: vec![("x-ratelimit-reset", reset.to_string())],
            body: json!({ "errors": [{ "type": "RATE_LIMITED", "message": "API rate limit exceeded" }] })
                .to_string(),
        },
        Canned {
            status: 403,
            headers: vec![
                ("x-ratelimit-remaining", "0".into()),
                ("x-ratelimit-reset", reset.to_string()),
            ],
            body: json!({ "message": "API rate limit exceeded for user." }).to_string(),
        },
        answer(401, json!({ "message": "Bad credentials" })),
        answer(500, json!({ "message": "Server Error" })),
    ]);
    set(
        "SWITCHBOARD_GITHUB_GRAPHQL",
        format!("{}/graphql", fake.url),
    );
    set("SWITCHBOARD_GITHUB_API", &fake.url);
    let q = |source| util::block_on(github::graphql("query { x }", json!({}), source));

    let err = q("a").unwrap_err();
    assert_eq!(kind(&err), ErrorKind::NotFound);
    assert!(err.to_string().contains("acme/nope"), "{err}");

    let err = q("b").unwrap_err();
    assert_eq!(
        kind(&err),
        ErrorKind::RateLimited {
            reset_at: Some(reset)
        }
    );

    let rest = |path| util::block_on(github::rest("GET", path, None, "c"));
    let err = rest("/x").unwrap_err();
    assert_eq!(
        kind(&err),
        ErrorKind::RateLimited {
            reset_at: Some(reset)
        }
    );
    assert!(err.to_string().contains("API rate limit exceeded"), "{err}");

    let err = rest("/y").unwrap_err();
    assert_eq!(kind(&err), ErrorKind::Unauthorized);
    assert!(err.to_string().contains("GH_TOKEN"), "{err}");
    assert!(err.to_string().contains("Bad credentials"), "{err}");

    let err = rest("/z").unwrap_err();
    assert_eq!(kind(&err), ErrorKind::Failed);
    assert!(err.to_string().contains("500"), "{err}");
    assert_eq!(counted("GitHub", "GraphQL") + counted("GitHub", "REST"), 5);
}

#[test]
fn a_service_that_cannot_be_reached_is_still_counted() {
    let _env = Env::new("gh-down");
    set("GH_TOKEN", "tok-4");
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    set("SWITCHBOARD_GITHUB_API", format!("http://127.0.0.1:{port}"));
    let err = util::block_on(github::rest("GET", "/x", None, "test")).unwrap_err();
    assert_eq!(kind(&err), ErrorKind::Unreachable);
    assert!(
        err.to_string().starts_with("Couldn't reach GitHub"),
        "{err}"
    );
    assert_eq!(counted("GitHub", "REST"), 1);
}

#[test]
fn turned_off_nothing_is_asked() {
    let env = Env::new("off");
    set("GH_TOKEN", "tok-5");
    set("LINEAR_API_KEY", "lin_api_5");
    env.settings(json!({ "github": false, "linear": false }));
    let err = util::block_on(github::graphql("query { x }", json!({}), "test")).unwrap_err();
    assert_eq!(kind(&err), ErrorKind::Off);
    assert!(err.to_string().contains("turned off in Settings"), "{err}");
    let err = util::block_on(linear::ticket("ENG-1", "test")).unwrap_err();
    assert_eq!(kind(&err), ErrorKind::Off);
    assert_eq!(requests::tally().total, 0);
    env.settings(json!({}));
}

#[test]
fn without_a_token_it_says_how_to_store_one() {
    let env = Env::new("no-token");
    let err = util::block_on(github::token()).unwrap_err();
    assert_eq!(kind(&err), ErrorKind::NoCredentials);
    assert!(
        err.to_string()
            .contains("switchboard platform secret-set github-token"),
        "{err}"
    );
    let err = util::block_on(linear::api_key()).unwrap_err();
    assert_eq!(kind(&err), ErrorKind::NoCredentials);
    assert!(err.to_string().contains("linear-api-key"), "{err}");

    // The stored secret: on a Mac the Keychain's, through security (a
    // stand-in here, never your Keychain); elsewhere, with no keyring, the
    // file only you can read.
    if switchboard::platform::NAME == "macOS" {
        let security = env.home.join("bin/security");
        std::fs::write(
            &security,
            "#!/bin/sh\ncase \"$*\" in *github-token*) echo from-file ;; *) exit 44 ;; esac\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&security, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::create_dir_all(env.home.join(".config/switchboard")).unwrap();
    std::fs::write(
        env.home.join(".config/switchboard/secrets.json"),
        json!({ "github-token": "from-file\n" }).to_string(),
    )
    .unwrap();
    // linear.json is the key's last place, as worktree-from-ticket kept it.
    std::fs::write(
        env.home.join(".config/switchboard/linear.json"),
        json!({ "linearApiKey": "lin_api_file" }).to_string(),
    )
    .unwrap();
    assert_eq!(util::block_on(github::token()).unwrap(), "from-file");
    assert_eq!(util::block_on(linear::api_key()).unwrap(), "lin_api_file");
    // The variables come first.
    set("GH_TOKEN", "from-env");
    assert_eq!(util::block_on(github::token()).unwrap(), "from-env");
}

fn pr_node(number: u64, state: &str, owner: &str, head: &str) -> Value {
    json!({
        "number": number,
        "url": format!("https://github.com/acme/widgets/pull/{number}"),
        "title": format!("PR {number}"),
        "state": state,
        "isDraft": false,
        "baseRefName": "main",
        "headRefName": head,
        "headRepositoryOwner": { "login": owner },
    })
}

#[test]
fn the_pr_for_a_branch() {
    let env = Env::new("pr");
    set("GH_TOKEN", "tok-6");
    let dir = env.repo("widgets", "git@github.com:acme/widgets.git", "eng-42-thing");
    let fake = Fake::start(vec![answer(
        200,
        json!({ "data": { "repository": { "pullRequests": { "nodes": [
            pr_node(9, "CLOSED", "acme", "eng-42-thing"),
            pr_node(7, "OPEN", "acme", "eng-42-thing"),
        ] } } } }),
    )]);
    set(
        "SWITCHBOARD_GITHUB_GRAPHQL",
        format!("{}/graphql", fake.url),
    );
    assert_eq!(
        util::block_on(github::repo_of(&dir)).unwrap().to_string(),
        "acme/widgets"
    );
    let pr = util::block_on(github::pr_for_branch(&dir, "test"))
        .unwrap()
        .unwrap();
    assert_eq!(pr.number, 7);
    assert_eq!(pr.state, "OPEN");
    assert_eq!(pr.head_ref_name, "eng-42-thing");
    let seen = fake.seen();
    assert_eq!(
        seen[0].body["variables"],
        json!({ "owner": "acme", "name": "widgets", "head": "eng-42-thing" })
    );
    // Written out, it reads as `gh pr view --json` did.
    let written = serde_json::to_value(&pr).unwrap();
    assert_eq!(written["isDraft"], false);
    assert_eq!(written["baseRefName"], "main");
}

#[test]
fn a_fork_s_pr_is_found_by_the_branch_it_tracks() {
    let env = Env::new("fork");
    set("GH_TOKEN", "tok-7");
    let dir = env.repo(
        "widgets",
        "https://github.com/acme/widgets.git",
        "someone-feature",
    );
    env.git(
        &dir,
        &[
            "remote",
            "add",
            "fork",
            "https://github.com/someone/widgets.git",
        ],
    );
    env.git(&dir, &["config", "branch.someone-feature.remote", "fork"]);
    env.git(
        &dir,
        &[
            "config",
            "branch.someone-feature.merge",
            "refs/heads/feature",
        ],
    );
    let fake = Fake::start(vec![answer(
        200,
        json!({ "data": { "repository": { "pullRequests": { "nodes": [
            pr_node(11, "OPEN", "acme", "feature"),
            pr_node(10, "OPEN", "someone", "feature"),
        ] } } } }),
    )]);
    set(
        "SWITCHBOARD_GITHUB_GRAPHQL",
        format!("{}/graphql", fake.url),
    );
    let pr = util::block_on(github::pr_for_branch(&dir, "test"))
        .unwrap()
        .unwrap();
    assert_eq!(pr.number, 10);
    assert_eq!(fake.seen()[0].body["variables"]["head"], "feature");
}

#[test]
fn a_stranger_s_pr_from_the_same_branch_name_is_not_the_branch_s() {
    let env = Env::new("stranger");
    set("GH_TOKEN", "tok-9");
    let dir = env.repo("widgets", "git@github.com:acme/widgets.git", "main");
    let mut stranger = pr_node(30, "OPEN", "someone", "main");
    stranger["isCrossRepository"] = json!(true);
    let fake = Fake::start(vec![answer(
        200,
        json!({ "data": { "repository": { "pullRequests": { "nodes": [stranger] } } } }),
    )]);
    set(
        "SWITCHBOARD_GITHUB_GRAPHQL",
        format!("{}/graphql", fake.url),
    );
    assert_eq!(
        util::block_on(github::pr_for_branch(&dir, "test")).unwrap(),
        None
    );
}

#[test]
fn a_fork_s_clone_looks_in_the_project_upstream_names() {
    let env = Env::new("upstream");
    set("GH_TOKEN", "tok-10");
    let dir = env.repo("widgets", "git@github.com:me/widgets.git", "feature");
    env.git(
        &dir,
        &[
            "remote",
            "add",
            "upstream",
            "https://github.com/acme/widgets.git",
        ],
    );
    env.git(&dir, &["config", "branch.feature.remote", "origin"]);
    env.git(
        &dir,
        &["config", "branch.feature.merge", "refs/heads/feature"],
    );
    let mut mine = pr_node(41, "OPEN", "me", "feature");
    mine["isCrossRepository"] = json!(true);
    let fake = Fake::start(vec![
        answer(200, json!({ "data": { "repository": { "isFork": true } } })),
        answer(
            200,
            json!({ "data": { "repository": { "pullRequests": { "nodes": [mine] } } } }),
        ),
    ]);
    set(
        "SWITCHBOARD_GITHUB_GRAPHQL",
        format!("{}/graphql", fake.url),
    );
    let pr = util::block_on(github::pr_for_branch(&dir, "test"))
        .unwrap()
        .unwrap();
    assert_eq!(pr.number, 41);
    let seen = fake.seen();
    assert_eq!(
        seen[0].body["variables"],
        json!({ "owner": "me", "name": "widgets" })
    );
    assert_eq!(seen[1].body["variables"]["owner"], "acme");
    // `gh repo set-default` wins over both, with no question asked.
    env.git(&dir, &["config", "remote.origin.gh-resolved", "base"]);
    assert_eq!(
        util::block_on(github::repo_of(&dir)).unwrap().to_string(),
        "me/widgets"
    );
}

#[test]
fn a_wtpr_directory_is_looked_up_by_its_number() {
    let env = Env::new("wtpr");
    set("GH_TOKEN", "tok-8");
    let dir = env.repo("pr-12", "git@github.com:acme/widgets.git", "x");
    let mut node = pr_node(12, "MERGED", "acme", "x");
    node.as_object_mut().unwrap().remove("headRepositoryOwner");
    let fake = Fake::start(vec![
        answer(
            200,
            json!({ "data": { "repository": { "pullRequest": node } } }),
        ),
        answer(
            200,
            json!({ "data": { "repository": { "pullRequest": null } }, "errors": [{
                "type": "NOT_FOUND",
                "message": "Could not resolve to a PullRequest with the number of 12." }] }),
        ),
    ]);
    set(
        "SWITCHBOARD_GITHUB_GRAPHQL",
        format!("{}/graphql", fake.url),
    );
    let pr = util::block_on(github::pr_for_branch(&dir, "test"))
        .unwrap()
        .unwrap();
    assert_eq!((pr.number, pr.state.as_str()), (12, "MERGED"));
    assert_eq!(fake.seen()[0].body["variables"]["number"], 12);
    assert_eq!(
        util::block_on(github::pr_for_branch(&dir, "test")).unwrap(),
        None
    );
}

#[test]
fn a_linear_ticket() {
    let _env = Env::new("linear");
    set("LINEAR_API_KEY", "lin_api_x");
    let fake = Fake::start(vec![
        answer(
            200,
            json!({ "data": { "issue": {
                "id": "uuid-1", "identifier": "ENG-1", "title": "Fix the login",
                "url": "https://linear.app/acme/issue/ENG-1",
                "state": { "name": "In Progress" },
                "assignee": { "id": "u1", "name": "jo", "displayName": "Jo" } } } }),
        ),
        answer(
            400,
            json!({ "errors": [{ "message": "Entity not found: Issue",
                "extensions": { "code": "INPUT_ERROR", "type": "invalid input" } }] }),
        ),
        answer(
            400,
            json!({ "errors": [{ "message": "Authentication required, not authenticated",
                "extensions": { "code": "AUTHENTICATION_ERROR" } }] }),
        ),
    ]);
    set("SWITCHBOARD_LINEAR_API", &fake.url);
    let ticket = util::block_on(linear::ticket("ENG-1", "test: ticket"))
        .unwrap()
        .unwrap();
    assert_eq!(ticket.title, "Fix the login");
    assert_eq!(ticket.state, "In Progress");
    assert_eq!(ticket.assignee.unwrap().name, "Jo");
    let seen = fake.seen();
    // A personal key goes as it is, with no "Bearer".
    assert_eq!(seen[0].header("authorization"), Some("lin_api_x"));
    assert_eq!(seen[0].body["variables"], json!({ "id": "ENG-1" }));
    assert!(
        seen[0].body["query"]
            .as_str()
            .unwrap()
            .contains("issue(id: $id)")
    );

    assert_eq!(
        util::block_on(linear::ticket("RELEASE-2024", "test")).unwrap(),
        None
    );
    let err = util::block_on(linear::ticket("ENG-2", "test")).unwrap_err();
    assert_eq!(kind(&err), ErrorKind::Unauthorized);
    assert!(err.to_string().contains("LINEAR_API_KEY"), "{err}");
    assert_eq!(counted("Linear", "GraphQL"), 3);
}
