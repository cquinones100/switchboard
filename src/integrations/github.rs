//! GitHub, over its own API rather than the `gh` command, so nothing has to
//! be installed: GraphQL and REST calls with a token, the repository a
//! directory belongs to, and the pull request for its branch, as
//! `tmux/window-pr.sh` found it with `gh pr list` and `gh pr view`, looked
//! for in origin and, in a fork's clone, the project its `upstream` remote
//! names (`branch_pr`).
//!
//! The token is the one the person already has, first that answers:
//! `GH_TOKEN`, `GITHUB_TOKEN` (the variables `gh` itself reads), `gh auth
//! token` when gh happens to be installed and signed in, else the stored
//! secret `github-token`. What gh or the secret gave is kept ten minutes,
//! since each look is a process, and forgotten as soon as GitHub refuses it.
//!
//! Every call refuses while the `github` setting is off (but in a program
//! you run yourself, `super::run_by_you`), and is counted in
//! the request audit under GitHub, as "GraphQL" or "REST".

use std::fmt;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Result, anyhow};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Credential, Remembered, ensure_on, env_value, stored_secret};
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

/// How long `gh auth token` may take. It only reads gh's own config or the
/// keyring, so a gh that takes longer is stuck, and the secret is tried.
const GH_TIMEOUT: Duration = Duration::from_secs(5);

/// The token to send, as the module's comment says where it is looked for.
pub async fn token() -> Result<String> {
    Ok(credential().await?.value)
}

async fn credential() -> Result<Credential> {
    for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Some(value) = env_value(var) {
            return Ok(Credential {
                value,
                from: var.to_string(),
            });
        }
    }
    if let Some(known) = REMEMBERED.get() {
        return Ok(known);
    }
    let found = match from_gh().await {
        Some(value) => Some(Credential {
            value,
            from: "gh".to_string(),
        }),
        None => stored_secret("github-token").await.map(|value| Credential {
            value,
            from: "the stored secret github-token".to_string(),
        }),
    };
    match found {
        Some(credential) => {
            REMEMBERED.set(&credential);
            Ok(credential)
        }
        None => Err(ServiceError::new(
            ErrorKind::NoCredentials,
            SERVICE,
            "No GitHub token: set GH_TOKEN, sign in with `gh auth login` if you use gh, or \
             store a token with `switchboard platform secret-set github-token` (it reads the \
             token from stdin).",
        )
        .into()),
    }
}

// gh's own token, when gh is there and signed in. Its absence, or a gh that
// isn't signed in, is no error: the secret is looked at next.
async fn from_gh() -> Option<String> {
    util::which("gh")?;
    let mut cmd = tokio::process::Command::new("gh");
    cmd.args(["auth", "token"]);
    let output = util::run_async(&mut cmd, Some(GH_TIMEOUT)).await.ok()?;
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!token.is_empty()).then_some(token)
}

/// Forgets the token gh or the secret gave, so the next request looks again:
/// for after the person signs in anew.
pub fn forget_token() {
    REMEMBERED.forget();
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
    let auth = format!("Bearer {}", credential.value);
    let headers = [
        ("Authorization", auth.as_str()),
        ("Accept", accept),
        ("X-GitHub-Api-Version", "2022-11-28"),
    ];
    let audit = Audit {
        service: SERVICE,
        protocol,
        source,
    };
    let response = net::request_json(method, url, &headers, body, audit).await?;
    Ok((response, credential))
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
                     login`, or store a new one with `switchboard platform secret-set \
                     github-token`.",
                    credential.from,
                    if message.is_empty() { "401" } else { &message }
                ),
            )
        }
        403 | 429
            if response.header("x-ratelimit-remaining") == Some("0")
                || response.header("retry-after").is_some()
                || message.to_lowercase().contains("rate limit") =>
        {
            rate_limited(response, &message)
        }
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
pub fn pr_number_from_dir(dir: &Path) -> Option<u64> {
    let name = dir.file_name()?.to_str()?;
    let rest = name.strip_prefix("pr-")?;
    let number = rest.split('-').next()?;
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
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
        let repo = repo_of(dir).await?;
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
    let branch = git(dir, &["branch", "--show-current"])
        .await
        .unwrap_or_default();
    if branch.is_empty() {
        return Ok(None);
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
    let owner = pushed_to(dir, &branch)
        .await
        .unwrap_or_else(|| repo.owner.clone());
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

// The owner of the repository a branch is pushed to: its remote's URL (the
// remote may be a URL itself, as `gh pr checkout` sets it). `None` for a
// branch with no remote, or one not on GitHub.
async fn pushed_to(dir: &Path, branch: &str) -> Option<String> {
    let remote = git(dir, &["config", &format!("branch.{branch}.remote")])
        .await
        .ok()
        .filter(|r| !r.is_empty())?;
    let url = match git(dir, &["remote", "get-url", &remote]).await {
        Ok(url) => url,
        Err(_) => remote,
    };
    parse_remote(&url).map(|r| r.owner)
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
