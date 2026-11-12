//! GitHub, over its own API with gh's sign-in rather than through gh's own
//! commands: GraphQL and REST calls with a token, the repository a
//! directory belongs to, and the pull request for its branch, as
//! `tmux/window-pr.sh` found it with `gh pr list` and `gh pr view`, looked
//! for in origin and, in a fork's clone, the project its `upstream` remote
//! names (`branch_pr`).
//!
//! The token is gh's, from `gh auth token`, and from nowhere else: every
//! call to GitHub's API, the update check's among them (`get_public`), signs
//! in as the account `gh auth login` signed gh in to, so Switchboard and gh
//! always agree on who you are and share one rate limit. gh decides which
//! token that is, its own variables (`GH_TOKEN`) included. What gh gave is
//! kept ten minutes, since each look is a process, and forgotten as soon as
//! GitHub refuses it. One look runs at a time, the others waiting on its
//! answer, and a look that found nothing (a gh signed out, or too slow on a
//! busy computer) isn't made again for a minute, the token found before still
//! used meanwhile, so a stuck gh is one process a minute rather than one each
//! lookup.
//!
//! Every call but `get_public` refuses while the `github` setting is off (but
//! in a program you run yourself, `super::run_by_you`); `get_public` goes
//! without a token then. Every call is counted in the request audit under
//! GitHub, as "GraphQL" or "REST", and every answer to a call with a token
//! says how much of the hour's limit is left, kept for the lookups to slow
//! down by (`tightest_limit`).

use std::fmt;
use std::path::Path;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Credential, Remembered, ensure_on};
use crate::net::{self, Audit, ErrorKind, Response, ServiceError};
use crate::util;

const SERVICE: &str = "GitHub";

/// The REST API's address (`SWITCHBOARD_GITHUB_API` moves it).
pub fn api_url() -> String {
    net::base_url("SWITCHBOARD_GITHUB_API", "https://api.github.com")
}

/// The GraphQL endpoint (`SWITCHBOARD_GITHUB_GRAPHQL` moves it).
pub fn graphql_url() -> String {
    net::base_url(
        "SWITCHBOARD_GITHUB_GRAPHQL",
        "https://api.github.com/graphql",
    )
}

// ── The token ───────────────────────────────────────────────────────────────

static REMEMBERED: Remembered = Remembered::new();

// The look under way, which the others wait on rather than run gh too.
static LOOKING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// When a look last found nothing, in ms; 0 once one finds a token or the
// token is forgotten.
static FOUND_NOTHING_AT: AtomicI64 = AtomicI64::new(0);

/// How long a look that found nothing stands before the next one.
const LOOK_AGAIN_MS: i64 = 60 * 1000;

/// How long `gh auth token` may take. It only reads gh's own config or the
/// keyring, so a gh that takes longer is stuck, and counts as finding none.
const GH_TIMEOUT: Duration = Duration::from_secs(5);

/// gh's token, as the module's comment says.
pub async fn token() -> Result<String> {
    Ok(credential().await?.value)
}

async fn credential() -> Result<Credential> {
    if let Some(known) = REMEMBERED.get() {
        return Ok(known);
    }
    let _one_look = LOOKING.lock().await;
    if let Some(known) = REMEMBERED.get() {
        return Ok(known);
    }
    let found_nothing_lately =
        util::now_ms() - FOUND_NOTHING_AT.load(Ordering::Relaxed) < LOOK_AGAIN_MS;
    if found_nothing_lately {
        return REMEMBERED.last().ok_or_else(no_token);
    }
    let found = from_gh().await.map(|value| Credential {
        value,
        from: "gh".to_string(),
    });
    match found {
        Some(credential) => {
            FOUND_NOTHING_AT.store(0, Ordering::Relaxed);
            REMEMBERED.set(&credential);
            Ok(credential)
        }
        None => {
            FOUND_NOTHING_AT.store(util::now_ms(), Ordering::Relaxed);
            REMEMBERED.last().ok_or_else(no_token)
        }
    }
}

fn no_token() -> anyhow::Error {
    ServiceError::new(
        ErrorKind::NoCredentials,
        SERVICE,
        "No GitHub token: Switchboard signs in to GitHub as GitHub's CLI does, so install \
         gh (https://cli.github.com) and sign in with `gh auth login`.",
    )
    .into()
}

// gh's own token, when gh is there and signed in; `None` for a gh that isn't
// installed, isn't signed in or doesn't answer in time.
async fn from_gh() -> Option<String> {
    util::which("gh")?;
    let mut cmd = tokio::process::Command::new("gh");
    cmd.args(["auth", "token"]);
    let output = util::run_async(&mut cmd, Some(GH_TIMEOUT)).await.ok()?;
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!token.is_empty()).then_some(token)
}

/// Forgets the token gh gave, so the next request looks again at once: for
/// after the person signs in anew.
pub fn forget_token() {
    REMEMBERED.forget();
    FOUND_NOTHING_AT.store(0, Ordering::Relaxed);
}

// ── Requests ────────────────────────────────────────────────────────────────

async fn send(
    method: &str,
    url: &str,
    body: Option<&Value>,
    protocol: &str,
    source: &str,
) -> Result<(Response, Credential)> {
    send_accepting(
        method,
        url,
        body,
        protocol,
        source,
        "application/vnd.github+json",
    )
    .await
}

async fn send_accepting(
    method: &str,
    url: &str,
    body: Option<&Value>,
    protocol: &str,
    source: &str,
    accept: &str,
) -> Result<(Response, Credential)> {
    ensure_on("github", SERVICE)?;
    let credential = credential().await?;
    let response = request(
        method,
        url,
        body,
        protocol,
        source,
        accept,
        Some(&credential),
    )
    .await?;
    Ok((response, credential))
}

// One request to GitHub's API, with what every one carries: the token, when
// there is one, what to answer in, and the API's version.
async fn request(
    method: &str,
    url: &str,
    body: Option<&Value>,
    protocol: &str,
    source: &str,
    accept: &str,
    credential: Option<&Credential>,
) -> Result<Response> {
    let auth = credential.map(|c| format!("Bearer {}", c.value));
    let mut headers = vec![("Accept", accept), ("X-GitHub-Api-Version", "2022-11-28")];
    if let Some(auth) = &auth {
        headers.push(("Authorization", auth.as_str()));
    }
    let audit = Audit {
        service: SERVICE,
        protocol,
        source,
    };
    let response = net::request_json(method, url, &headers, body, audit).await?;
    if credential.is_some() {
        note_limit(&response, protocol);
    }
    Ok(response)
}

// ── The hour's limit ────────────────────────────────────────────────────────

/// What GitHub last said of one of its limits (`graphql`, `core`, ...): how
/// many points or requests are left of how many, and when it resets, in
/// seconds since the epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limit {
    pub remaining: i64,
    pub limit: i64,
    pub reset: i64,
}

impl Limit {
    /// What is left of the hour, from 0 to 1.
    pub fn left(&self) -> f64 {
        if self.limit <= 0 {
            return 1.0;
        }
        self.remaining.max(0) as f64 / self.limit as f64
    }
}

// Every limit's last reading, by its resource, shared by every call.
static LIMITS: std::sync::Mutex<Vec<(String, Limit)>> = std::sync::Mutex::new(Vec::new());

/// The limit an answer's headers report, with its resource (GitHub's
/// `x-ratelimit-resource`, else `graphql` for a GraphQL call and `core` for
/// a REST one); none when they don't say.
pub fn limit_of(response: &Response, protocol: &str) -> Option<(String, Limit)> {
    let number = |name: &str| response.header(name)?.trim().parse::<i64>().ok();
    let limit = Limit {
        remaining: number("x-ratelimit-remaining")?,
        limit: number("x-ratelimit-limit")?,
        reset: number("x-ratelimit-reset")?,
    };
    let resource = response
        .header("x-ratelimit-resource")
        .map(str::to_string)
        .unwrap_or_else(|| {
            if protocol == "GraphQL" {
                "graphql".into()
            } else {
                "core".into()
            }
        });
    Some((resource, limit))
}

fn note_limit(response: &Response, protocol: &str) {
    let Some((resource, limit)) = limit_of(response, protocol) else {
        return;
    };
    let mut limits = LIMITS.lock().unwrap_or_else(|e| e.into_inner());
    match limits.iter_mut().find(|(r, _)| *r == resource) {
        Some((_, kept)) => *kept = limit,
        None => limits.push((resource, limit)),
    }
}

/// The hour's limit with the least left, GraphQL's or REST's (`core`; the
/// search API's own limit is a minute's), of those GitHub's answers
/// reported that haven't reset since: what slows the lookups down
/// (`sources::schedule::Budget`), the health light's line and the Usage
/// view's. None while every one has reset, or none was read.
pub fn tightest_limit() -> Option<Limit> {
    let now = util::now_secs();
    let limits = LIMITS.lock().unwrap_or_else(|e| e.into_inner());
    limits
        .iter()
        .filter(|(r, _)| r == "graphql" || r == "core")
        .map(|(_, l)| *l)
        .filter(|l| l.reset > now)
        .min_by(|a, b| a.left().total_cmp(&b.left()))
}

/// A REST GET of what anyone may read (a public repository's releases),
/// which GitHub answers without a token too, but then from the 60 requests an
/// hour shared by every program behind the same address, which others spend.
/// So it carries gh's token when gh has one, and counts against your own
/// 5,000 an hour; it goes without one when gh has none, and while the
/// `github` setting is off, which keeps your account out of what the
/// workspace does by itself. When GitHub refuses gh's token, or answers that
/// your account's limit is spent (the token kept, since it is still good),
/// it asks again at once without it, as it would have before gh's token was
/// sent. The answer whatever its status, for the caller to read.
pub async fn get_public(path: &str, source: &str) -> Result<Response> {
    let url = format!("{}/{}", api_url(), path.trim_start_matches('/'));
    let credential = match ensure_on("github", SERVICE) {
        Ok(()) => credential().await.ok(),
        Err(_) => None,
    };
    let accept = "application/vnd.github+json";
    let response = request(
        "GET",
        &url,
        None,
        "REST",
        source,
        accept,
        credential.as_ref(),
    )
    .await?;
    if credential.is_none() {
        return Ok(response);
    }
    let refused = response.status == 401;
    if refused {
        forget_token();
    }
    if refused || is_rate_limited(&response, &message_of(&response)) {
        return request("GET", &url, None, "REST", source, accept, None).await;
    }
    Ok(response)
}

/// A GraphQL query's `data`. GitHub's errors in the answer (a repository it
/// doesn't have, the rate limit spent) are errors here, with its message.
/// `source` says what asked, for the request audit.
pub async fn graphql(query: &str, variables: Value, source: &str) -> Result<Value> {
    let body = json!({ "query": query, "variables": variables });
    let (response, credential) =
        send("POST", &graphql_url(), Some(&body), "GraphQL", source).await?;
    check_status(&response, &credential)?;
    if let Some(errors) = response.body.get("errors").and_then(Value::as_array)
        && !errors.is_empty()
    {
        return Err(graphql_error(errors, &response).into());
    }
    match response.body.get("data") {
        Some(data) if !data.is_null() => Ok(data.clone()),
        _ => Err(ServiceError::new(
            ErrorKind::Failed,
            SERVICE,
            "GitHub's GraphQL answer had no data.",
        )
        .into()),
    }
}

/// A GraphQL query that asks about several things at once (a repository
/// each, a PR each), whose answer may hold `data` for some and `errors` for
/// others: GitHub answers for the rest when one repository or PR can't be
/// found, and each error's `path` names the alias it was about. The data and
/// the errors, so a caller keeps the answers it got; an error only when there
/// is no data at all.
pub async fn graphql_partial(
    query: &str,
    variables: Value,
    source: &str,
) -> Result<(Value, Vec<Value>)> {
    graphql_answer(query, variables, source)
        .await
        .map(|(data, errors, _)| (data, errors))
}

/// Every node of a connection GitHub sends a page at a time, and whether
/// they are all there: false when `cap` pages were read and GitHub said
/// there were more.
#[derive(Clone, Debug, PartialEq)]
pub struct Paged {
    pub nodes: Vec<Value>,
    pub complete: bool,
}

/// The rest of a connection whose first page (its `nodes` and `pageInfo {
/// hasNextPage endCursor }`) is `first`: `query` asked again with `after`
/// set to the last page's `endCursor`, its connection found in the answer's
/// `data` at the pointer `connection`, until GitHub says there are no more
/// or `cap` pages (the first among them) are read. Every node, in order,
/// the first page's first; each page is a `graphql` call of its own, so the
/// request audit counts it as it counts the first.
pub async fn rest_of(
    query: &str,
    variables: &Value,
    connection: &str,
    first: &Value,
    cap: usize,
    source: &str,
) -> Result<Paged> {
    pages_after(first, cap, |after| {
        let mut asked = variables.clone();
        asked["after"] = Value::String(after);
        async move {
            let data = graphql(query, asked, source).await?;
            Ok(data.pointer(connection).cloned().unwrap_or(Value::Null))
        }
    })
    .await
}

// `rest_of`'s paging, with how a page after a cursor is read given, so it
// is tested without GitHub.
async fn pages_after<F, Fut>(first: &Value, cap: usize, mut next: F) -> Result<Paged>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = Result<Value>>,
{
    let nodes_of = |page: &Value| {
        page.get("nodes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let mut nodes = nodes_of(first);
    let mut page = first.clone();
    let mut read = 1;
    loop {
        let more = page
            .pointer("/pageInfo/hasNextPage")
            .and_then(Value::as_bool)
            == Some(true);
        let cursor = page.pointer("/pageInfo/endCursor").and_then(Value::as_str);
        let (true, Some(cursor)) = (more, cursor) else {
            return Ok(Paged {
                nodes,
                complete: true,
            });
        };
        if read >= cap {
            return Ok(Paged {
                nodes,
                complete: false,
            });
        }
        page = next(cursor.to_string()).await?;
        nodes.extend(nodes_of(&page));
        read += 1;
    }
}

// `graphql_partial`'s answer, with the response its errors came in, for a
// caller that takes some of them as failing the whole (`graphql_error`).
async fn graphql_answer(
    query: &str,
    variables: Value,
    source: &str,
) -> Result<(Value, Vec<Value>, Response)> {
    let body = json!({ "query": query, "variables": variables });
    let (response, credential) =
        send("POST", &graphql_url(), Some(&body), "GraphQL", source).await?;
    check_status(&response, &credential)?;
    let errors = response
        .body
        .get("errors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    match response.body.get("data") {
        Some(data) if !data.is_null() => Ok((data.clone(), errors, response)),
        _ if !errors.is_empty() => Err(graphql_error(&errors, &response).into()),
        _ => Err(ServiceError::new(
            ErrorKind::Failed,
            SERVICE,
            "GitHub's GraphQL answer had no data.",
        )
        .into()),
    }
}

/// A REST call: `path` under the API's address (`/repos/owner/name`), `body`
/// sent as JSON. The answer's body, `null` for one with none (204).
pub async fn rest(method: &str, path: &str, body: Option<&Value>, source: &str) -> Result<Value> {
    let url = format!("{}/{}", api_url(), path.trim_start_matches('/'));
    let (response, credential) = send(method, &url, body, "REST", source).await?;
    check_status(&response, &credential)?;
    Ok(response.body)
}

/// A REST GET whose answer carries GitHub's rendered HTML beside the
/// Markdown (`body_html`, for the PR image view), as `gh api -H "Accept:
/// application/vnd.github.full+json"` read it.
pub async fn rest_rendered(path: &str, source: &str) -> Result<Value> {
    let url = format!("{}/{}", api_url(), path.trim_start_matches('/'));
    let (response, credential) = send_accepting(
        "GET",
        &url,
        None,
        "REST",
        source,
        "application/vnd.github.full+json",
    )
    .await?;
    check_status(&response, &credential)?;
    Ok(response.body)
}

// GitHub's own words for a failure: REST's `message`, else the body's text.
fn message_of(response: &Response) -> String {
    match &response.body {
        Value::Object(o) => o
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        Value::String(text) => text
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(200)
            .collect(),
        _ => String::new(),
    }
}

// When the rate limit comes back, in seconds since the epoch: the reset
// header, else now plus `retry-after` (the secondary limits give only that).
fn reset_of(response: &Response) -> Option<i64> {
    let reset = response
        .header("x-ratelimit-reset")
        .and_then(|v| v.trim().parse::<i64>().ok());
    let retry = response
        .header("retry-after")
        .and_then(|v| v.trim().parse::<i64>().ok())
        .map(|after| util::now_secs() + after);
    reset.or(retry)
}

fn rate_limited(response: &Response, message: &str) -> ServiceError {
    let reset_at = reset_of(response);
    let when = match reset_at {
        Some(at) => {
            let minutes = ((at - util::now_secs()).max(0) + 59) / 60;
            format!(" It comes back in about {minutes} min.")
        }
        None => String::new(),
    };
    ServiceError::new(
        ErrorKind::RateLimited { reset_at },
        SERVICE,
        format!(
            "GitHub's rate limit is spent{}.{when}",
            if message.is_empty() {
                String::new()
            } else {
                format!(": {message}")
            }
        ),
    )
}

// Whether a 403 or 429 is GitHub's rate limit, its primary or a secondary
// one, rather than a refusal of what was asked.
fn is_rate_limited(response: &Response, message: &str) -> bool {
    matches!(response.status, 403 | 429)
        && (response.header("x-ratelimit-remaining") == Some("0")
            || response.header("retry-after").is_some()
            || message.to_lowercase().contains("rate limit"))
}

fn check_status(response: &Response, credential: &Credential) -> Result<()> {
    if response.ok() {
        return Ok(());
    }
    let message = message_of(response);
    let status = response.status;
    let error = match status {
        401 => {
            forget_token();
            ServiceError::new(
                ErrorKind::Unauthorized,
                SERVICE,
                format!(
                    "GitHub refused the token from {} ({}): sign in again with `gh auth \
                     login`.",
                    credential.from,
                    if message.is_empty() { "401" } else { &message }
                ),
            )
        }
        403 | 429 if is_rate_limited(response, &message) => rate_limited(response, &message),
        404 => ServiceError::new(
            ErrorKind::NotFound,
            SERVICE,
            format!(
                "GitHub has no such thing, or the token can't see it{}.",
                if message.is_empty() {
                    String::new()
                } else {
                    format!(" ({message})")
                }
            ),
        ),
        _ => ServiceError::new(
            ErrorKind::Failed,
            SERVICE,
            format!(
                "GitHub answered {status}{}",
                if message.is_empty() {
                    String::new()
                } else {
                    format!(": {message}")
                }
            ),
        ),
    };
    Err(error.into())
}

fn graphql_error(errors: &[Value], response: &Response) -> ServiceError {
    let messages: Vec<&str> = errors
        .iter()
        .filter_map(|e| e.get("message").and_then(Value::as_str))
        .collect();
    let message = if messages.is_empty() {
        "an error it didn't explain".to_string()
    } else {
        messages.join("; ")
    };
    let has = |kind: &str| {
        errors
            .iter()
            .any(|e| e.get("type").and_then(Value::as_str) == Some(kind))
    };
    if has("RATE_LIMITED") {
        rate_limited(response, &message)
    } else if has("NOT_FOUND") {
        ServiceError::new(ErrorKind::NotFound, SERVICE, format!("GitHub: {message}"))
    } else {
        ServiceError::new(ErrorKind::Failed, SERVICE, format!("GitHub: {message}"))
    }
}

// ── Repositories ────────────────────────────────────────────────────────────

/// A repository on GitHub, `owner/name`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Repo {
    pub owner: String,
    pub name: String,
}

impl fmt::Display for Repo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

impl Repo {
    /// Whether it is `other`, as GitHub reads names: whatever their case.
    pub fn same(&self, other: &Repo) -> bool {
        self.owner.eq_ignore_ascii_case(&other.owner) && self.name.eq_ignore_ascii_case(&other.name)
    }
}

// An owner or repository name as GitHub allows them, which also keeps
// anything else out of the REST paths they go into.
static PART: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._-]+$").unwrap());

// Whether a remote's host is GitHub's: github.com and its ssh and www hosts,
// or an ssh alias for it of the kind kept for a second account
// (`github.com-work`, `github-personal`), which ~/.ssh/config maps to
// github.com. A host with a dot after such a prefix is a server of its own
// (GitHub Enterprise), whose API isn't api.github.com, so it isn't taken.
fn github_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if matches!(
        host.as_str(),
        "github.com" | "ssh.github.com" | "www.github.com"
    ) {
        return true;
    }
    ["github.com-", "github-", "github_"].iter().any(|prefix| {
        host.strip_prefix(prefix)
            .is_some_and(|rest| !rest.contains('.'))
    })
}

/// The repository a remote URL names, in any form git takes for GitHub:
/// `git@github.com:owner/name.git`, `ssh://git@github.com/owner/name`,
/// `https://github.com/owner/name.git` (with a user or token before the
/// host), `git://...`. `None` for a remote elsewhere.
pub fn parse_remote(url: &str) -> Option<Repo> {
    let url = url.trim();
    let (host, path) = if let Some((_, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        // A port (`ssh.github.com:443`), and IPv6's brackets, are not the host.
        let host = host.split(':').next().unwrap_or(host);
        (host, path)
    } else {
        // scp's form, `[user@]host:path`, which git reads only when the
        // colon comes before any slash.
        let (authority, path) = url.split_once(':')?;
        if authority.contains('/') {
            return None;
        }
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        (host, path)
    };
    if !github_host(host) {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return None;
    };
    (PART.is_match(owner) && PART.is_match(name) && name != "." && name != "..").then(|| Repo {
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

const GIT_TIMEOUT: Duration = Duration::from_secs(10);

// One git command in `dir`, its output trimmed. `--no-optional-locks`, as the
// tab lookups ran it, so a lookup never holds the index lock a commit in a
// pane wants.
async fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C").arg(dir).arg("--no-optional-locks").args(args);
    let output = util::run_async(&mut cmd, Some(GIT_TIMEOUT)).await?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The GitHub repository `dir`'s pull requests are in, picked as `gh`
/// picked the repository it asked: the one `gh repo set-default` named (git's
/// `remote.<name>.gh-resolved`), else an `upstream` remote when there is one
/// and `origin` is a fork, else `origin`. In a fork's clone, where origin is
/// your fork and upstream the project, that is the project, where PRs from
/// your fork are opened. A branch's PR is looked for in both
/// (`branch_pr`), since it may be in either.
pub async fn repo_of(dir: &Path) -> Result<Repo> {
    match remotes(dir).await? {
        Remotes::Default(repo) => Ok(repo),
        Remotes::Clone {
            origin,
            upstream: Some(upstream),
        } if is_fork(&origin).await? => Ok(upstream),
        Remotes::Clone { origin, .. } => Ok(origin),
    }
}

// The repositories a clone's PRs may be in, as its git config names them,
// before GitHub is asked anything.
enum Remotes {
    // The one `gh repo set-default` named, which gh asks alone.
    Default(Repo),
    // Origin, and the `upstream` remote's when it names another repository
    // on GitHub (in a fork's clone, the project).
    Clone {
        origin: Repo,
        upstream: Option<Repo>,
    },
}

async fn remotes(dir: &Path) -> Result<Remotes> {
    if git(dir, &["rev-parse", "--git-dir"]).await.is_err() {
        return Err(anyhow!(
            "{} isn't in a git repository.",
            crate::paths::tilde(dir)
        ));
    }
    let resolved = git(
        dir,
        &["config", "--get-regexp", r"^remote\..*\.gh-resolved$"],
    )
    .await
    .unwrap_or_default();
    if let Some((remote, value)) = set_default(&resolved) {
        if value != "base"
            && let Some(repo) = parse_name_with_owner(&value)
        {
            return Ok(Remotes::Default(repo));
        }
        if let Some(repo) = remote_repo(dir, &remote).await {
            return Ok(Remotes::Default(repo));
        }
    }
    let url = git(dir, &["remote", "get-url", "origin"])
        .await
        .map_err(|_| anyhow!("This repository has no origin remote."))?;
    let origin = parse_remote(&url)
        .ok_or_else(|| anyhow!("This repository's origin isn't on GitHub: {url}"))?;
    let upstream = remote_repo(dir, "upstream")
        .await
        .filter(|upstream| !upstream.same(&origin));
    Ok(Remotes::Clone { origin, upstream })
}

// The remote `gh repo set-default` marked, and what it wrote there: `base`,
// or (as older gh wrote it) the repository itself, `owner/name`.
fn set_default(config: &str) -> Option<(String, String)> {
    config.lines().find_map(|line| {
        let (key, value) = line.split_once(' ')?;
        let remote = key.strip_prefix("remote.")?.strip_suffix(".gh-resolved")?;
        let value = value.trim();
        (!remote.is_empty() && !value.is_empty()).then(|| (remote.to_string(), value.to_string()))
    })
}

/// A repository from its `owner/name`, as GitHub writes `nameWithOwner`.
pub fn parse_name_with_owner(text: &str) -> Option<Repo> {
    let (owner, name) = text.split_once('/')?;
    (PART.is_match(owner) && PART.is_match(name) && !name.contains('/')).then(|| Repo {
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

// The GitHub repository a remote's URL names.
async fn remote_repo(dir: &Path, remote: &str) -> Option<Repo> {
    let url = git(dir, &["remote", "get-url", remote]).await.ok()?;
    parse_remote(&url)
}

// Whether GitHub has the repository as a fork, kept an hour, since a fork
// stays one and `repo_of` asks on every look.
static FORKS: LazyLock<std::sync::Mutex<std::collections::HashMap<Repo, (bool, i64)>>> =
    LazyLock::new(Default::default);

const FORK_KEEP_MS: i64 = 60 * 60 * 1000;

async fn is_fork(repo: &Repo) -> Result<bool> {
    let known = FORKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(repo)
        .filter(|(_, at)| util::now_ms() - at < FORK_KEEP_MS)
        .map(|(fork, _)| *fork);
    if let Some(fork) = known {
        return Ok(fork);
    }
    let data = graphql(
        "query($owner: String!, $name: String!) { repository(owner: $owner, name: $name) { isFork } }",
        json!({ "owner": repo.owner, "name": repo.name }),
        "the repository's PRs: is origin a fork",
    )
    .await?;
    let fork = data
        .pointer("/repository/isFork")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    FORKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(repo.clone(), (fork, util::now_ms()));
    Ok(fork)
}

/// The repository's default branch, as GitHub has it.
pub async fn default_branch(repo: &Repo, source: &str) -> Result<String> {
    let data = graphql(
        "query($owner: String!, $name: String!) { repository(owner: $owner, name: $name) { defaultBranchRef { name } } }",
        json!({ "owner": repo.owner, "name": repo.name }),
        source,
    )
    .await?;
    data.pointer("/repository/defaultBranchRef/name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            ServiceError::new(
                ErrorKind::NotFound,
                SERVICE,
                format!("GitHub has no default branch for {repo}: it may be empty."),
            )
            .into()
        })
}

// ── Pull requests ───────────────────────────────────────────────────────────

/// A pull request, with the fields `gh pr view --json` gave under the same
/// names, so what is written from it reads as before.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
    pub title: String,
    /// "OPEN", "MERGED" or "CLOSED".
    pub state: String,
    pub is_draft: bool,
    pub base_ref_name: String,
    pub head_ref_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Node {
    #[serde(flatten)]
    pr: PullRequest,
    #[serde(default)]
    is_cross_repository: Option<bool>,
    head_repository_owner: Option<Login>,
}

#[derive(Deserialize)]
struct Login {
    login: String,
}

const FIELDS: &str = "number url title state isDraft baseRefName headRefName";

/// The PR number a `wtpr` worktree's directory carries (`pr-123`, or
/// `pr-123-...`), which needs no lookup to know. `pr-12abc` is no number.
/// A `wtpr --untrusted` worktree, `<owner>-<repo>/123` in the review
/// folder (src/trust.rs), on a branch that names no PR on GitHub, carries
/// it too.
pub fn pr_number_from_dir(dir: &Path) -> Option<u64> {
    pr_number_from_dir_of(&crate::paths::home(), dir)
}

/// The repository a `wtpr --untrusted` worktree's PR is in, which its
/// folder names (`<owner>-<name>/<n>`, src/trust.rs's `review_folder`) and
/// its `pr-review/<n>` branch can't: origin's or upstream's, whichever it
/// is, read from git's files alone. `None` for any other directory.
pub fn reviewed_repo(dir: &Path) -> Option<Repo> {
    if !crate::trust::in_review_dir(dir) {
        return None;
    }
    let folder = dir.parent()?.file_name()?.to_str()?;
    let remotes = crate::sources::repo::remotes_of(dir);
    [remotes.upstream, remotes.origin]
        .into_iter()
        .flatten()
        .find(|r| crate::trust::review_folder(&r.owner, &r.name) == folder)
}

/// `pr_number_from_dir` under another home.
pub fn pr_number_from_dir_of(home: &Path, dir: &Path) -> Option<u64> {
    let name = dir.file_name()?.to_str()?;
    let digits = |n: &str| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit());
    let number = match name.strip_prefix("pr-") {
        Some(rest) => rest.split('-').next()?,
        None => {
            let folder = dir.parent()?.parent()?;
            let review = crate::trust::review_dir_in(home);
            let in_review = digits(name)
                && (folder == review || review.canonicalize().is_ok_and(|r| folder == r));
            if !in_review {
                return None;
            }
            name
        }
    };
    if !digits(number) {
        return None;
    }
    number.parse().ok()
}

/// The pull request for the branch checked out in `dir`, or `None` when the
/// branch has none (or `dir` is on no branch): `branch_pr`'s, for a caller
/// that needs nothing of the repository it is in.
pub async fn pr_for_branch(dir: &Path, source: &str) -> Result<Option<PullRequest>> {
    Ok(branch_pr(dir, source).await?.map(|(_, pr)| pr))
}

/// The pull request for the branch checked out in `dir`, with the
/// repository it is in, or `None` when the branch has none (or `dir` is on
/// no branch). As `tmux/window-pr.sh`: a merged or closed PR still counts,
/// since the work in the tab is the same work it was before it merged,
/// though an open one is taken first; and a `wtpr` directory's number is
/// looked up by that number, in `repo_of`'s repository, as `wtpr` took it.
///
/// It is looked for in the repository `gh repo set-default` named, as gh
/// asked that alone; else in origin and, in a fork's clone, the project its
/// `upstream` remote names, both in one query, origin's PR winning when
/// both have one, so it is found whichever the branch's PR was opened
/// against. Only a PR from where the branch is pushed counts, as `gh pr
/// view` matched one: the repository its remote names (your fork, in a
/// fork's clone; a fork's PR checked out as `<owner>-<branch>`, looked up by
/// the branch it tracks), else origin's. A stranger's PR that happens to have
/// the same head branch (`main`, `patch-1`), or another fork's into
/// upstream, is never the branch's. Upstream being a repository your token
/// can't see only leaves it out; any other failure of its part fails the
/// lookup when origin has no PR, so a PR already known isn't taken as gone.
pub async fn branch_pr(dir: &Path, source: &str) -> Result<Option<(Repo, PullRequest)>> {
    if let Some(number) = pr_number_from_dir(dir) {
        let repo = match reviewed_repo(dir) {
            Some(repo) => repo,
            None => repo_of(dir).await?,
        };
        let pr = pr_by_number(&repo, number, source).await?;
        return Ok(pr.map(|pr| (repo, pr)));
    }
    branch_pr_by_branch(dir, source).await
}

/// `branch_pr` by the branch alone, a `wtpr` directory's too: its PR is the
/// one from the branch it tracks, in whichever repository that was opened
/// in, though a bare number would be read as another's.
pub async fn branch_pr_by_branch(dir: &Path, source: &str) -> Result<Option<(Repo, PullRequest)>> {
    let (repo, upstream) = match remotes(dir).await? {
        Remotes::Default(repo) => (repo, None),
        Remotes::Clone { origin, upstream } => (origin, upstream),
    };
    let Some((branch, head)) = branch_and_head(dir).await else {
        return Ok(None);
    };
    let owner = pushed_to(dir, &branch)
        .await
        .map_or_else(|| repo.owner.clone(), |r| r.owner);
    let prs = format!(
        "pullRequests(headRefName: $head, first: 20, orderBy: {{field: CREATED_AT, direction: DESC}}) {{ \
         nodes {{ {FIELDS} isCrossRepository headRepositoryOwner {{ login }} }} }}"
    );
    // Upstream's part failing for another reason than being out of the
    // token's sight, kept until origin's answer is known.
    let mut upstream_failed: Option<ServiceError> = None;
    let data = match &upstream {
        None => {
            graphql(
                &format!(
                    "query($owner: String!, $name: String!, $head: String!) {{ \
                     repository(owner: $owner, name: $name) {{ {prs} }} }}"
                ),
                json!({ "owner": repo.owner, "name": repo.name, "head": head }),
                source,
            )
            .await?
        }
        Some(up) => {
            let (data, errors, response) = graphql_answer(
                &format!(
                    "query($owner: String!, $name: String!, $head: String!, \
                     $upstreamOwner: String!, $upstreamName: String!) {{ \
                     repository(owner: $owner, name: $name) {{ {prs} }} \
                     upstream: repository(owner: $upstreamOwner, name: $upstreamName) {{ {prs} }} }}"
                ),
                json!({
                    "owner": repo.owner, "name": repo.name, "head": head,
                    "upstreamOwner": up.owner, "upstreamName": up.name,
                }),
                source,
            )
            .await?;
            let (upstream_errors, failing): (Vec<Value>, Vec<Value>) = errors
                .into_iter()
                .partition(|e| e.pointer("/path/0").and_then(Value::as_str) == Some("upstream"));
            if !failing.is_empty() {
                return Err(graphql_error(&failing, &response).into());
            }
            let out_of_sight = |e: &Value| {
                matches!(
                    e.get("type").and_then(Value::as_str),
                    Some("NOT_FOUND" | "FORBIDDEN")
                )
            };
            if !upstream_errors.iter().all(out_of_sight) {
                upstream_failed = Some(graphql_error(&upstream_errors, &response));
            }
            data
        }
    };
    let nodes = |at: &str| -> Vec<Node> {
        data.pointer(&format!("/{at}/pullRequests/nodes"))
            .and_then(Value::as_array)
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|n| serde_json::from_value(n.clone()).ok())
                    .collect()
            })
            .unwrap_or_default()
    };
    if let Some(pr) = choose(nodes("repository"), &owner, &repo.owner) {
        return Ok(Some((repo, pr)));
    }
    if let Some(failed) = upstream_failed {
        return Err(failed.into());
    }
    Ok(upstream.and_then(|up| {
        let pr = choose(nodes("upstream"), &owner, &up.owner)?;
        Some((up, pr))
    }))
}

// The branch `dir` is on and its name where it is pushed: the branch it
// tracks there, else its own. `None` on a detached HEAD.
async fn branch_and_head(dir: &Path) -> Option<(String, String)> {
    let branch = git(dir, &["branch", "--show-current"])
        .await
        .unwrap_or_default();
    if branch.is_empty() {
        return None;
    }
    let merge = git(dir, &["config", &format!("branch.{branch}.merge")])
        .await
        .unwrap_or_default();
    let tracked = merge.strip_prefix("refs/heads/").unwrap_or(&merge);
    let head = if tracked.is_empty() {
        branch.clone()
    } else {
        tracked.to_string()
    };
    Some((branch, head))
}

/// The branch `dir` is on as GitHub has it: the repository it is pushed to
/// (its push remote's, else its remote's, else origin's, or the one `gh repo set-default` named)
/// and its name there (the branch it tracks, else its own), as
/// `branch_pr_by_branch` matches its PR. `None` on a detached HEAD.
pub async fn pushed_branch(dir: &Path) -> Result<Option<(Repo, String)>> {
    let repo = match remotes(dir).await? {
        Remotes::Default(repo) => repo,
        Remotes::Clone { origin, .. } => origin,
    };
    let Some((branch, head)) = branch_and_head(dir).await else {
        return Ok(None);
    };
    // A push remote of its own (`branch.<name>.pushRemote`, else
    // `remote.pushDefault`) other than the one it tracks is where `git push`
    // sends the branch, under its own name. Otherwise it goes where it is
    // tracked, to the branch it tracks unless `push.default` pushes every
    // branch under its own name.
    let own_name = matches!(
        git(dir, &["config", "push.default"])
            .await
            .unwrap_or_default()
            .as_str(),
        "current" | "matching"
    );
    let tracked = git(dir, &["config", &format!("branch.{branch}.remote")])
        .await
        .unwrap_or_default();
    for key in [
        format!("branch.{branch}.pushRemote"),
        "remote.pushDefault".into(),
    ] {
        let remote = git(dir, &["config", &key]).await.unwrap_or_default();
        if remote.is_empty() {
            continue;
        }
        if remote == tracked {
            break;
        }
        let url = git(dir, &["remote", "get-url", "--push", &remote])
            .await
            .unwrap_or(remote);
        if let Some(pushed) = parse_remote(&url) {
            return Ok(Some((pushed, branch)));
        }
    }
    let name = if own_name { branch.clone() } else { head };
    // A branch with no remote of its own is pushed to origin, whichever
    // repository `gh repo set-default` named.
    let pushed = match pushed_to(dir, &branch).await {
        Some(pushed) => pushed,
        None => remote_repo(dir, "origin").await.unwrap_or(repo),
    };
    Ok(Some((pushed, name)))
}

// The repository a branch is pushed to: its remote's URL (the remote may be
// a URL itself, as `gh pr checkout` sets it). `None` for a branch with no
// remote, or one not on GitHub.
async fn pushed_to(dir: &Path, branch: &str) -> Option<Repo> {
    let remote = git(dir, &["config", &format!("branch.{branch}.remote")])
        .await
        .ok()
        .filter(|r| !r.is_empty())?;
    let url = match git(dir, &["remote", "get-url", &remote]).await {
        Ok(url) => url,
        Err(_) => remote,
    };
    parse_remote(&url)
}

// The PR to show among those on the head branch, newest first: one whose
// head is in `owner`'s repository, as `gh pr view` matched the head's
// `owner:branch` (just the branch, from a PR within the repository itself,
// when `owner` is `base_owner`), an open one before the rest.
fn choose(nodes: Vec<Node>, owner: &str, base_owner: &str) -> Option<PullRequest> {
    let within = owner.eq_ignore_ascii_case(base_owner);
    let matches = |n: &&Node| {
        let from_owner = n
            .head_repository_owner
            .as_ref()
            .is_some_and(|have| have.login.eq_ignore_ascii_case(owner));
        match n.is_cross_repository {
            Some(cross) => from_owner && cross != within,
            None => from_owner,
        }
    };
    let pool: Vec<&Node> = nodes.iter().filter(matches).collect();
    pool.iter()
        .find(|n| n.pr.state == "OPEN")
        .or_else(|| pool.first())
        .map(|n| n.pr.clone())
}

/// A pull request by its number, or `None` when the repository has none.
pub async fn pr_by_number(repo: &Repo, number: u64, source: &str) -> Result<Option<PullRequest>> {
    let result = graphql(
        &format!(
            "query($owner: String!, $name: String!, $number: Int!) {{ repository(owner: $owner, name: $name) {{ \
             pullRequest(number: $number) {{ {FIELDS} }} }} }}"
        ),
        json!({ "owner": repo.owner, "name": repo.name, "number": number }),
        source,
    )
    .await;
    let data = match result {
        Ok(data) => data,
        // A number with no PR is GitHub's NOT_FOUND on the pullRequest field
        // ("Could not resolve to a PullRequest with the number of 12"): an
        // answer, not a failure, where a repository it can't find is one.
        Err(e)
            if net::kind_of(&e) == Some(&ErrorKind::NotFound)
                && e.to_string().contains("PullRequest") =>
        {
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    Ok(data
        .pointer("/repository/pullRequest")
        .filter(|pr| !pr.is_null())
        .and_then(|pr| serde_json::from_value(pr.clone()).ok()))
}

/// An open PR from a fork, and who opened it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkPr {
    pub pr: PullRequest,
    pub author: String,
}

/// The repository's open PRs from forks, the most recently updated first
/// (the newest hundred PRs looked through).
pub async fn open_fork_prs(repo: &Repo, source: &str) -> Result<Vec<ForkPr>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Open {
        #[serde(flatten)]
        pr: PullRequest,
        is_cross_repository: bool,
        author: Option<Login>,
    }
    let data = graphql(
        &format!(
            "query($owner: String!, $name: String!) {{ repository(owner: $owner, name: $name) {{ \
             pullRequests(states: OPEN, first: 100, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{ \
             nodes {{ {FIELDS} isCrossRepository author {{ login }} }} }} }} }}"
        ),
        json!({ "owner": repo.owner, "name": repo.name }),
        source,
    )
    .await?;
    Ok(data
        .pointer("/repository/pullRequests/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|n| serde_json::from_value::<Open>(n.clone()).ok())
        .filter(|n| n.is_cross_repository)
        .map(|n| ForkPr {
            pr: n.pr,
            author: n.author.map(|a| a.login).unwrap_or_default(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_s_headers_give_its_limit_by_resource() {
        let answer = |headers: &[(&str, &str)]| Response {
            status: 200,
            headers: headers
                .iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect(),
            body: Value::Null,
        };
        let counted = [
            ("x-ratelimit-remaining", "900"),
            ("x-ratelimit-limit", "5000"),
            ("x-ratelimit-reset", "1800000000"),
        ];
        let (resource, limit) = limit_of(&answer(&counted), "GraphQL").unwrap();
        assert_eq!(resource, "graphql", "named by the protocol when unsaid");
        assert_eq!(limit.remaining, 900);
        assert!((limit.left() - 0.18).abs() < 1e-9);
        let mut named = counted.to_vec();
        named.push(("X-RateLimit-Resource", "search"));
        assert_eq!(limit_of(&answer(&named), "GraphQL").unwrap().0, "search");
        assert_eq!(limit_of(&answer(&counted), "REST").unwrap().0, "core");
        assert_eq!(limit_of(&answer(&counted[..2]), "REST"), None, "no reset");
    }

    fn page(names: &[&str], next: Option<&str>) -> Value {
        json!({
            "nodes": names.iter().map(|n| json!({"name": n})).collect::<Vec<_>>(),
            "pageInfo": {"hasNextPage": next.is_some(), "endCursor": next},
        })
    }

    fn names(paged: &Paged) -> Vec<&str> {
        paged
            .nodes
            .iter()
            .map(|n| n["name"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn the_pages_after_the_first_are_read_in_order_with_each_cursor() {
        let mut asked = Vec::new();
        let paged = util::block_on(pages_after(&page(&["a", "b"], Some("c1")), 10, |after| {
            asked.push(after.clone());
            let answer = match after.as_str() {
                "c1" => page(&["c"], Some("c2")),
                _ => page(&["d"], None),
            };
            async move { Ok(answer) }
        }))
        .unwrap();
        assert_eq!(names(&paged), ["a", "b", "c", "d"]);
        assert!(paged.complete);
        assert_eq!(asked, ["c1", "c2"]);
    }

    #[test]
    fn a_single_page_asks_nothing_and_the_cap_stops_the_rest() {
        let paged = util::block_on(pages_after(&page(&["a"], None), 10, |_| async {
            panic!("asked for a second page")
        }))
        .unwrap();
        assert_eq!(names(&paged), ["a"]);
        assert!(paged.complete);

        let mut asked = 0;
        let paged = util::block_on(pages_after(&page(&["a"], Some("c")), 3, |_| {
            asked += 1;
            async { Ok(page(&["more"], Some("c"))) }
        }))
        .unwrap();
        assert_eq!(asked, 2, "three pages, the first among them");
        assert_eq!(names(&paged), ["a", "more", "more"]);
        assert!(!paged.complete);
    }

    fn repo(owner: &str, name: &str) -> Option<Repo> {
        Some(Repo {
            owner: owner.into(),
            name: name.into(),
        })
    }

    #[test]
    fn remotes_in_every_form() {
        for url in [
            "git@github.com:acme/widgets.git",
            "git@github.com:acme/widgets",
            "github.com:acme/widgets.git",
            "ssh://git@github.com/acme/widgets.git",
            "ssh://git@ssh.github.com:443/acme/widgets.git",
            "git+ssh://git@github.com/acme/widgets",
            "https://github.com/acme/widgets.git",
            "https://github.com/acme/widgets/",
            "https://x-access-token:abc@github.com/acme/widgets.git",
            "http://www.github.com/acme/widgets",
            "git://github.com/acme/widgets.git",
            "git@github.com-work:acme/widgets.git",
            "git@github-personal:acme/widgets.git",
            "  https://GitHub.com/acme/widgets.git\n",
        ] {
            assert_eq!(parse_remote(url), repo("acme", "widgets"), "{url}");
        }
    }

    #[test]
    fn remotes_elsewhere_are_not_github() {
        for url in [
            "git@gitlab.com:acme/widgets.git",
            "https://bitbucket.org/acme/widgets",
            "https://github.example.com/acme/widgets.git",
            "git@github.mycorp.com:acme/widgets.git",
            "https://github.com/acme",
            "https://github.com/acme/widgets/tree/main",
            "/home/me/widgets",
            "../widgets",
            "https://github.com/acme/../x",
            "https://github.com/ac me/widgets",
            "",
        ] {
            assert_eq!(parse_remote(url), None, "{url}");
        }
    }

    #[test]
    fn dotted_names_survive() {
        assert_eq!(
            parse_remote("git@github.com:acme/widgets.js.git"),
            repo("acme", "widgets.js")
        );
        assert_eq!(
            parse_remote("https://github.com/acme/.github"),
            repo("acme", ".github")
        );
    }

    #[test]
    fn wtpr_directories_carry_their_number() {
        assert_eq!(pr_number_from_dir(Path::new("/w/pr-123")), Some(123));
        assert_eq!(pr_number_from_dir(Path::new("/w/pr-123-fix-it")), Some(123));
        assert_eq!(pr_number_from_dir(Path::new("/w/pr-12abc")), None);
        assert_eq!(pr_number_from_dir(Path::new("/w/pr-")), None);
        assert_eq!(pr_number_from_dir(Path::new("/w/eng-12")), None);
        let home = Path::new("/h");
        let review = crate::trust::review_dir_in(home);
        assert_eq!(
            pr_number_from_dir_of(home, &review.join("acme-widgets/7")),
            Some(7),
            "wtpr --untrusted's worktree"
        );
        assert_eq!(
            pr_number_from_dir_of(home, Path::new("/w/acme-widgets/7")),
            None
        );
        assert_eq!(
            pr_number_from_dir_of(home, &review.join("acme-widgets/7/src")),
            None
        );
    }

    fn node(number: u64, state: &str, owner: &str) -> Node {
        Node {
            pr: PullRequest {
                number,
                url: format!("https://github.com/acme/widgets/pull/{number}"),
                title: "t".into(),
                state: state.into(),
                is_draft: false,
                base_ref_name: "main".into(),
                head_ref_name: "b".into(),
            },
            is_cross_repository: Some(!owner.eq_ignore_ascii_case("acme")),
            head_repository_owner: Some(Login {
                login: owner.into(),
            }),
        }
    }

    #[test]
    fn an_open_pr_is_taken_before_a_newer_closed_one() {
        let nodes = vec![node(9, "CLOSED", "acme"), node(5, "OPEN", "acme")];
        assert_eq!(choose(nodes, "acme", "acme").map(|p| p.number), Some(5));
        let nodes = vec![node(9, "MERGED", "acme"), node(5, "CLOSED", "acme")];
        assert_eq!(choose(nodes, "acme", "acme").map(|p| p.number), Some(9));
    }

    #[test]
    fn the_owner_decides_between_same_named_branches() {
        let nodes = vec![node(9, "OPEN", "someone"), node(5, "OPEN", "Acme")];
        assert_eq!(choose(nodes, "acme", "acme").map(|p| p.number), Some(5));
        // A stranger's PR from their own `main` is never the branch's.
        let nodes = vec![node(9, "OPEN", "someone")];
        assert_eq!(choose(nodes, "acme", "acme"), None);
        // A branch pushed to a fork takes only the fork's own.
        let nodes = vec![node(9, "OPEN", "someone"), node(7, "OPEN", "fork")];
        assert_eq!(choose(nodes, "fork", "acme").map(|p| p.number), Some(7));
        // Nor, pushed to the repository itself, a PR from another of the
        // owner's repositories.
        let mut other = node(4, "OPEN", "acme");
        other.is_cross_repository = Some(true);
        assert_eq!(choose(vec![other], "acme", "acme"), None);
    }

    #[test]
    fn set_default_is_read_from_git_config() {
        assert_eq!(
            set_default("remote.upstream.gh-resolved base\n"),
            Some(("upstream".into(), "base".into()))
        );
        assert_eq!(
            set_default("remote.origin.gh-resolved acme/widgets"),
            Some(("origin".into(), "acme/widgets".into()))
        );
        assert_eq!(set_default(""), None);
        assert_eq!(
            parse_name_with_owner("acme/widgets"),
            repo("acme", "widgets")
        );
        assert_eq!(parse_name_with_owner("base"), None);
    }
}
