//! GitHub and Linear as the rest of Switchboard reaches them, against a small
//! HTTP server of the test's own on 127.0.0.1 (each service's address moved
//! there by its `SWITCHBOARD_*` variable): what is sent (the token, the
//! query), what is made of each kind of answer, and that every request is
//! counted in the request audit, which lands in a temporary HOME.
//!
//! The environment is the process's, so every test holds `ENV` for its whole
//! run, and the file is a binary of its own, apart from tests/it, whose other
//! tests' threads a change to the environment would race.

#![cfg(feature = "net")]

mod common;

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};

use common::http::{self, Reply, Service};
use common::stand_in;
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

fn answer(status: u16, body: Value) -> Reply {
    Reply::json(status, body)
}

/// GitHub or Linear on 127.0.0.1, as strict as the real one about how it is
/// asked (tests/common/http.rs: the method, the path, the token), answering
/// each request it takes with the next canned answer, on a connection of
/// its own.
struct Fake {
    url: String,
    http: http::Fake,
}

impl Fake {
    /// GitHub, whose token is `token`.
    fn github(token: &str, answers: Vec<Reply>) -> Fake {
        Fake::of(http::Fake::canned(
            Service::GitHub {
                token: token.into(),
            },
            answers,
        ))
    }

    /// Linear, whose API key is `key`.
    fn linear(key: &str, answers: Vec<Reply>) -> Fake {
        Fake::of(http::Fake::canned(
            Service::Linear { key: key.into() },
            answers,
        ))
    }

    fn of(http: http::Fake) -> Fake {
        Fake {
            url: http.url.clone(),
            http,
        }
    }

    fn seen(&self) -> Vec<Seen> {
        self.http
            .seen()
            .into_iter()
            .map(|r| Seen {
                body: r.json(),
                method: r.method,
                path: r.target,
                headers: r.headers,
            })
            .collect()
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
    let fake = Fake::github(
        "tok-1",
        vec![answer(
            200,
            json!({ "data": { "viewer": { "login": "me" } } }),
        )],
    );
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
    let fake = Fake::github(
        "tok-2",
        vec![answer(200, json!({ "default_branch": "trunk" }))],
    );
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
    let fake = Fake::github(
        "tok-3",
        vec![
        answer(
            200,
            json!({ "data": { "repository": null }, "errors": [{ "type": "NOT_FOUND",
                "message": "Could not resolve to a Repository with the name 'acme/nope'." }] }),
        ),
        answer(
            200,
            json!({ "errors": [{ "type": "RATE_LIMITED", "message": "API rate limit exceeded" }] }),
        )
        .header("x-ratelimit-reset", reset.to_string()),
        answer(
            403,
            json!({ "message": "API rate limit exceeded for user." }),
        )
        .header("x-ratelimit-remaining", "0")
        .header("x-ratelimit-reset", reset.to_string()),
        answer(401, json!({ "message": "Bad credentials" })),
        answer(500, json!({ "message": "Server Error" })),
    ],
    );
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
        let keychain = env.home.join("keychain");
        stand_in::security(&env.home.join("bin"), &keychain);
        stand_in::keychain_item(&keychain, "github-token", "", "from-file");
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
    // A look that found nothing stands a minute; forgetting looks again now.
    github::forget_token();
    assert_eq!(util::block_on(github::token()).unwrap(), "from-file");
    assert_eq!(util::block_on(linear::api_key()).unwrap(), "lin_api_file");
    // The variables come first.
    set("GH_TOKEN", "from-env");
    assert_eq!(util::block_on(github::token()).unwrap(), "from-env");
    stand_in::assert_none_refused(&env.home.join("bin"));
}

/// A stand-in gh in the test's PATH that writes a line to `runs` each time
/// `gh auth token` runs, then does `then` (sh).
fn gh_counting(env: &Env, then: &str) -> PathBuf {
    let runs = env.home.join("gh-runs");
    stand_in::Gh::new()
        .answer(&format!(
            "printf 'run\\n' >> {}\n{then}",
            stand_in::sh_quote(&runs.to_string_lossy())
        ))
        .install(&env.home.join("bin"));
    runs
}

fn lines_in(file: &Path) -> usize {
    std::fs::read_to_string(file).map_or(0, |t| t.lines().count())
}

#[test]
fn lookups_at_once_run_gh_once() {
    let env = Env::new("gh-once");
    let runs = gh_counting(&env, "printf 'gho_once\\n'");
    let tokens =
        util::block_on(async { tokio::join!(github::token(), github::token(), github::token()) });
    for token in [tokens.0, tokens.1, tokens.2] {
        assert_eq!(token.unwrap(), "gho_once");
    }
    assert_eq!(lines_in(&runs), 1);
    stand_in::assert_none_refused(&env.home.join("bin"));
}

#[test]
fn a_gh_that_gives_nothing_is_asked_again_only_after_a_minute() {
    let env = Env::new("gh-backoff");
    let runs = gh_counting(&env, "exit 1");
    for _ in 0..3 {
        let err = util::block_on(github::token()).unwrap_err();
        assert_eq!(kind(&err), ErrorKind::NoCredentials);
    }
    assert_eq!(lines_in(&runs), 1);
    stand_in::assert_none_refused(&env.home.join("bin"));
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
    let fake = Fake::github(
        "tok-6",
        vec![answer(
            200,
            json!({ "data": { "repository": { "pullRequests": { "nodes": [
            pr_node(9, "CLOSED", "acme", "eng-42-thing"),
            pr_node(7, "OPEN", "acme", "eng-42-thing"),
        ] } } } }),
        )],
    );
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
    let fake = Fake::github(
        "tok-7",
        vec![answer(
            200,
            json!({ "data": { "repository": { "pullRequests": { "nodes": [
            pr_node(11, "OPEN", "acme", "feature"),
            pr_node(10, "OPEN", "someone", "feature"),
        ] } } } }),
        )],
    );
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
    let fake = Fake::github(
        "tok-9",
        vec![answer(
            200,
            json!({ "data": { "repository": { "pullRequests": { "nodes": [stranger] } } } }),
        )],
    );
    set(
        "SWITCHBOARD_GITHUB_GRAPHQL",
        format!("{}/graphql", fake.url),
    );
    assert_eq!(
        util::block_on(github::pr_for_branch(&dir, "test")).unwrap(),
        None
    );
}

/// A fork's clone: origin `me/widgets`, upstream `acme/widgets`, on
/// `feature`, pushed to origin.
fn fork_clone(env: &Env, name: &str) -> PathBuf {
    let dir = env.repo(name, "git@github.com:me/widgets.git", "feature");
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
    dir
}

/// A PR into `base` from `owner`'s `feature`, as GitHub lists it there.
fn fork_pr(number: u64, base: &str, owner: &str) -> Value {
    let mut pr = pr_node(number, "OPEN", owner, "feature");
    pr["url"] = json!(format!("https://github.com/{base}/pull/{number}"));
    pr["isCrossRepository"] = json!(!base.starts_with(&format!("{owner}/")));
    pr
}

#[test]
fn a_fork_s_clone_takes_the_project_upstream_names_as_gh_does() {
    let env = Env::new("upstream");
    set("GH_TOKEN", "tok-10");
    let dir = fork_clone(&env, "widgets");
    let fake = Fake::github(
        "tok-10",
        vec![answer(
            200,
            json!({ "data": { "repository": { "isFork": true } } }),
        )],
    );
    set(
        "SWITCHBOARD_GITHUB_GRAPHQL",
        format!("{}/graphql", fake.url),
    );
    assert_eq!(
        util::block_on(github::repo_of(&dir)).unwrap().to_string(),
        "acme/widgets"
    );
    assert_eq!(
        fake.seen()[0].body["variables"],
        json!({ "owner": "me", "name": "widgets" })
    );
    // `gh repo set-default` wins over both, with no question asked.
    env.git(&dir, &["config", "remote.origin.gh-resolved", "base"]);
    assert_eq!(
        util::block_on(github::repo_of(&dir)).unwrap().to_string(),
        "me/widgets"
    );
}

#[test]
fn a_fork_s_branch_s_pr_is_found_in_the_project_upstream_names_and_no_other_fork_s() {
    let env = Env::new("upstream-pr");
    set("GH_TOKEN", "tok-11");
    let dir = fork_clone(&env, "widgets");
    // Upstream has two PRs from a `feature`: another fork's, newer, and
    // this fork's; origin has none.
    let fake = Fake::github(
        "tok-11",
        vec![answer(
            200,
            json!({ "data": {
                "repository": { "pullRequests": { "nodes": [] } },
                "upstream": { "pullRequests": { "nodes": [
                    fork_pr(52, "acme/widgets", "someone"),
                    fork_pr(41, "acme/widgets", "me"),
                ] } }
            } }),
        )],
    );
    set(
        "SWITCHBOARD_GITHUB_GRAPHQL",
        format!("{}/graphql", fake.url),
    );
    let (repo, pr) = util::block_on(github::branch_pr(&dir, "test"))
        .unwrap()
        .unwrap();
    assert_eq!((repo.to_string().as_str(), pr.number), ("acme/widgets", 41));
    // One question for both, and none of whether origin is a fork.
    let seen = fake.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].body["variables"],
        json!({
            "owner": "me", "name": "widgets", "head": "feature",
            "upstreamOwner": "acme", "upstreamName": "widgets",
        })
    );
    let query = seen[0].body["query"].as_str().unwrap();
    assert!(
        query.contains("upstream: repository(owner: $upstreamOwner, name: $upstreamName)"),
        "{query}"
    );
    assert_eq!(counted("GitHub", "GraphQL"), 1, "and counted");
}

#[test]
fn another_fork_s_branch_of_the_same_name_is_never_the_branch_s() {
    let env = Env::new("other-fork");
    set("GH_TOKEN", "tok-12");
    let dir = fork_clone(&env, "widgets");
    let fake = Fake::github(
        "tok-12",
        vec![answer(
            200,
            json!({ "data": {
                "repository": { "pullRequests": { "nodes": [] } },
                "upstream": { "pullRequests": { "nodes": [
                    fork_pr(52, "acme/widgets", "someone"),
                    fork_pr(50, "acme/widgets", "acme"),
                ] } }
            } }),
        )],
    );
    set(
        "SWITCHBOARD_GITHUB_GRAPHQL",
        format!("{}/graphql", fake.url),
    );
    assert_eq!(
        util::block_on(github::branch_pr(&dir, "test")).unwrap(),
        None
    );
}

#[test]
fn origin_s_pr_wins_over_upstream_s() {
    let env = Env::new("origin-wins");
    set("GH_TOKEN", "tok-13");
    let dir = fork_clone(&env, "widgets");
    let fake = Fake::github(
        "tok-13",
        vec![answer(
            200,
            json!({ "data": {
                "repository": { "pullRequests": { "nodes": [fork_pr(3, "me/widgets", "me")] } },
                "upstream": { "pullRequests": { "nodes": [fork_pr(41, "acme/widgets", "me")] } }
            } }),
        )],
    );
    set(
        "SWITCHBOARD_GITHUB_GRAPHQL",
        format!("{}/graphql", fake.url),
    );
    let (repo, pr) = util::block_on(github::branch_pr(&dir, "test"))
        .unwrap()
        .unwrap();
    assert_eq!((repo.to_string().as_str(), pr.number), ("me/widgets", 3));
}

#[test]
fn an_upstream_github_cannot_answer_for_leaves_origin_s_answer() {
    let env = Env::new("upstream-gone");
    set("GH_TOKEN", "tok-14");
    let dir = fork_clone(&env, "widgets");
    let fake = Fake::github(
        "tok-14",
        vec![
            // Upstream is a repository the token can't see: origin's
            // answer stands, and it has no PR.
            answer(
                200,
                json!({
                    "data": { "repository": { "pullRequests": { "nodes": [] } }, "upstream": null },
                    "errors": [{ "type": "NOT_FOUND", "path": ["upstream"],
                        "message": "Could not resolve to a Repository with the name 'acme/widgets'." }]
                }),
            ),
            // Origin GitHub can't find fails the lookup, as it always did.
            answer(
                200,
                json!({
                    "data": { "repository": null, "upstream": { "pullRequests": { "nodes": [] } } },
                    "errors": [{ "type": "NOT_FOUND", "path": ["repository"],
                        "message": "Could not resolve to a Repository with the name 'me/widgets'." }]
                }),
            ),
            // Upstream failing for another reason, origin having no PR:
            // the lookup fails, so a PR already known isn't taken as gone.
            answer(
                200,
                json!({
                    "data": { "repository": { "pullRequests": { "nodes": [] } }, "upstream": null },
                    "errors": [{ "path": ["upstream"], "message": "Something went wrong." }]
                }),
            ),
        ],
    );
    set(
        "SWITCHBOARD_GITHUB_GRAPHQL",
        format!("{}/graphql", fake.url),
    );
    assert_eq!(
        util::block_on(github::branch_pr(&dir, "test")).unwrap(),
        None
    );
    let err = util::block_on(github::branch_pr(&dir, "test")).unwrap_err();
    assert_eq!(kind(&err), ErrorKind::NotFound);
    // A service's failure, which a lookup keeps what was known through.
    let err = util::block_on(github::branch_pr(&dir, "test")).unwrap_err();
    kind(&err);
}

#[test]
fn a_wtpr_directory_is_looked_up_by_its_number() {
    let env = Env::new("wtpr");
    set("GH_TOKEN", "tok-8");
    let dir = env.repo("pr-12", "git@github.com:acme/widgets.git", "x");
    let mut node = pr_node(12, "MERGED", "acme", "x");
    node.as_object_mut().unwrap().remove("headRepositoryOwner");
    let fake = Fake::github(
        "tok-8",
        vec![
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
        ],
    );
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
    let fake = Fake::linear(
        "lin_api_x",
        vec![
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
        ],
    );
    set("SWITCHBOARD_LINEAR_API", fake.http.graphql_url());
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
