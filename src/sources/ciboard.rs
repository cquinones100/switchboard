//! Mission control's CI view, ported from lib/sidebar/ciboard.js: your open
//! pull requests whose checks are failing, in the repositories your tabs are
//! in (and a fork's upstream, as the PR board searches them), each with the
//! checks that failed and the line of its log that says why. One search for
//! the pull requests and their first hundred checks (and, for a failing PR
//! whose failures aren't among them, the rest a hundred a query, through
//! `github::rest_of`), then the log of each one's first
//! failing GitHub Actions job, read once (a finished job's log doesn't
//! change) and kept in `ci-logs/<job>.log` in the state directory for "Read
//! the failing log"; at most `MAX_LOGS` are read a look, and the logs of
//! jobs no longer failing go, but those of the default branch's failing CI
//! (src/sources/mainci.rs), which lists its items here too.

use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::text;
use crate::integrations::github;
use crate::items::{Item, State, Target};
use crate::paths;

pub const MAX_LOGS: usize = 15;
const FAILED: [&str; 5] = [
    "FAILURE",
    "TIMED_OUT",
    "ACTION_REQUIRED",
    "STARTUP_FAILURE",
    "ERROR",
];
const FIELDS: &str = "... on PullRequest { number title url headRefName repository { nameWithOwner } \
  commits(last: 1) { nodes { commit { statusCheckRollup { state contexts(first: 100) { \
    pageInfo { hasNextPage endCursor } nodes { \
    __typename ... on CheckRun { name conclusion detailsUrl } ... on StatusContext { context state targetUrl } \
  } } } } } } }";
/// One PR's checks a page at a time, for a failing PR whose failures
/// GitHub didn't put on the search's first page.
const CHECKS_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!, $after: String) { \
  repository(owner: $owner, name: $name) { pullRequest(number: $number) { \
  commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100, after: $after) { \
    pageInfo { hasNextPage endCursor } nodes { \
    __typename ... on CheckRun { name conclusion detailsUrl } ... on StatusContext { context state targetUrl } \
  } } } } } } } } }";
/// Where a PR's checks are, in the search's answer and in `CHECKS_QUERY`'s.
const CONTEXTS: &str = "/commits/nodes/0/commit/statusCheckRollup/contexts";
/// The most pages of a PR's checks read, a thousand checks.
const CHECK_PAGES: usize = 10;

/// A failing check: its name, its page, and its GitHub Actions job's id
/// when it is one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failing {
    pub name: String,
    pub url: Option<String>,
    pub job: Option<String>,
}

/// The failing checks of a pull request.
pub fn failing_of(pr: &Value) -> Vec<Failing> {
    static JOB: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"/actions/runs/\d+/job/(\d+)").unwrap());
    pr.pointer("/commits/nodes/0/commit/statusCheckRollup/contexts/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|c| {
            let outcome = c
                .get("conclusion")
                .filter(|v| !v.is_null())
                .or_else(|| c.get("state"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_uppercase();
            FAILED.contains(&outcome.as_str())
        })
        .map(|c| {
            let url = c
                .get("detailsUrl")
                .filter(|v| !v.is_null())
                .or_else(|| c.get("targetUrl"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let name = c
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| c.get("context").and_then(Value::as_str))
                .unwrap_or("check")
                .to_string();
            let job = url
                .as_deref()
                .and_then(|u| JOB.captures(u))
                .map(|m| m[1].to_string());
            Failing { name, url, job }
        })
        .collect()
}

/// The line of a log that says why it failed: the last that says so the way
/// tools do (FAIL, Error:, ✗, a traceback), else the last that mentions an
/// error or a failure, leaving out the lines that only say a command failed,
/// else its last line, without the timestamp Actions puts first.
pub fn why_from(log: &str) -> String {
    static COLOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").unwrap());
    static STAMP: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\d{4}-\d\d-\d\dT[\d:.]+Z\s?").unwrap());
    static MARK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"##\[(error|group|endgroup)\]").unwrap());
    // The runner's and package managers' own last words only say it failed.
    static NOISE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Process completed with exit code|ELIFECYCLE|Command failed with exit code|npm ERR! (A complete log|Lifecycle)|DeprecationWarning|^shell: ").unwrap()
    });
    static STRONG: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\bFAIL(ED)?\b|\b[A-Za-z]*Error:|^error\b|✗|Traceback|panicked at").unwrap()
    });
    static WEAK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(errors?|fail(ed|ure|s)?)\b|assertion|exception").unwrap()
    });
    let clean = COLOR.replace_all(log, "");
    let lines: Vec<String> = clean
        .split('\n')
        .map(|l| {
            MARK.replace_all(&STAMP.replace(l, ""), "")
                .trim()
                .to_string()
        })
        .filter(|l| !l.is_empty())
        .collect();
    let last = |re: &Regex| {
        lines
            .iter()
            .rev()
            .find(|l| re.is_match(l) && !NOISE.is_match(l))
            .cloned()
    };
    last(&STRONG)
        .or_else(|| last(&WEAK))
        .or_else(|| lines.last().cloned())
        .unwrap_or_default()
}

/// Where a job's log is kept.
pub fn log_file(job: &str) -> PathBuf {
    paths::state_dir()
        .join("ci-logs")
        .join(format!("{job}.log"))
}

/// A failing PR's row: its checks that failed and why.
pub fn item_of(pr: &Value, failing: &[Failing], why: Option<&str>) -> Option<Item> {
    let number = pr.get("number").and_then(Value::as_u64)?;
    let url = text(pr, "/url");
    let repo = text(pr, "/repository/nameWithOwner");
    let names: Vec<&str> = failing.iter().map(|f| f.name.as_str()).collect();
    let mut detail = format!("✗ {}", names.join(", "));
    if let Some(why) = why.filter(|w| !w.is_empty()) {
        detail.push_str(" · ");
        detail.push_str(why);
    }
    let first = failing.first();
    let job = failing.iter().find(|f| f.job.is_some());
    let (job, job_name) = (job.and_then(|f| f.job.clone()), job.map(|f| f.name.clone()));
    Some(
        Item::new(url, format!("#{number} {}", text(pr, "/title")))
            .detail(detail)
            .state(State::Attention)
            .target(Target::Url(url.to_string()))
            .data(json!({
                "ci": {
                    "number": number, "repo": repo, "url": url,
                    "branch": text(pr, "/headRefName"),
                    "check": first.map(|f| json!({ "name": f.name, "url": f.url, "job": f.job })),
                    "job": job,
                    "logged": why.is_some(),
                    "jobName": job_name,
                },
                "search": repo,
            })),
    )
}

// A job's log, from the file kept of it, else from GitHub, kept then.
pub(crate) async fn read_log(repo: &str, job: &str) -> Option<String> {
    let file = log_file(job);
    if let Ok(log) = tokio::fs::read_to_string(&file).await {
        return Some(log);
    }
    let answer = github::rest(
        "GET",
        &format!("repos/{repo}/actions/jobs/{job}/logs"),
        None,
        "sidebar: CI",
    )
    .await
    .ok()?;
    let log = match answer {
        Value::String(text) => text,
        Value::Null => return None,
        other => other.to_string(),
    };
    if let Some(dir) = file.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    let _ = crate::util::write_atomic(&file, &log);
    Some(log)
}

/// Whether a PR's checks past the search's first page are needed: GitHub
/// says its checks failed, none of the first page's did, and there are more.
fn needs_rest(pr: &Value) -> bool {
    let rollup = text(pr, "/commits/nodes/0/commit/statusCheckRollup/state");
    let more = pr
        .pointer(&format!("{CONTEXTS}/pageInfo/hasNextPage"))
        .and_then(Value::as_bool)
        == Some(true);
    FAILED.contains(&rollup) && more && failing_of(pr).is_empty()
}

/// Every check of a PR, the search's first page and the rest; none for a
/// PR the search didn't name a repository and number for.
async fn rest_of_checks(pr: &Value) -> anyhow::Result<Option<github::Paged>> {
    let (Some((owner, name)), Some(number), Some(first)) = (
        text(pr, "/repository/nameWithOwner").split_once('/'),
        pr.get("number").and_then(Value::as_u64),
        pr.pointer(CONTEXTS),
    ) else {
        return Ok(None);
    };
    let paged = github::rest_of(
        CHECKS_QUERY,
        &json!({ "owner": owner, "name": name, "number": number }),
        &format!("/repository/pullRequest{CONTEXTS}"),
        first,
        CHECK_PAGES,
        "sidebar: CI",
    )
    .await?;
    Ok(Some(paged))
}

/// The PR with every check read in place of the first page's.
fn with_checks(pr: &mut Value, paged: github::Paged) {
    if let Some(contexts) = pr.pointer_mut(CONTEXTS) {
        contexts["nodes"] = Value::Array(paged.nodes);
    }
}

/// Looks for your failing PRs in the repositories, and why each fails.
pub async fn fetch(repos: Vec<String>, also_kept: Vec<String>) -> anyhow::Result<Vec<Item>> {
    let query = format!(
        "query($q: String!) {{ search(query: $q, type: ISSUE, first: 50) {{ nodes {{ {FIELDS} }} }} }}"
    );
    let scope = super::prboard::scope_of(&repos);
    let data = github::graphql(
        &query,
        json!({ "q": format!("is:pr is:open author:@me {scope}") }),
        "sidebar: CI",
    )
    .await?;
    let mut prs = data
        .pointer("/search/nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // A page that can't be read fails the look, so the view keeps what the
    // last one found rather than dropping a PR whose failure it can't see.
    for pr in prs.iter_mut().filter(|p| needs_rest(p)) {
        if let Some(paged) = rest_of_checks(pr).await? {
            with_checks(pr, paged);
        }
    }
    let mut logs = 0;
    let mut items = Vec::new();
    for pr in prs.iter().filter(|p| p.get("number").is_some()) {
        let failing = failing_of(pr);
        if failing.is_empty() {
            continue;
        }
        let repo = text(pr, "/repository/nameWithOwner");
        let job = failing.iter().find_map(|f| f.job.clone());
        let why = match job {
            Some(job) if logs < MAX_LOGS => {
                logs += 1;
                read_log(repo, &job).await.map(|log| why_from(&log))
            }
            _ => None,
        };
        items.extend(item_of(pr, &failing, why.as_deref()));
    }
    // Logs of jobs no longer failing go.
    let keep: Vec<String> = items
        .iter()
        .filter_map(|i| i.data.pointer("/ci/job").and_then(Value::as_str))
        .chain(also_kept.iter().map(String::as_str))
        .map(|job| format!("{job}.log"))
        .collect();
    if let Ok(mut entries) = tokio::fs::read_dir(paths::state_dir().join("ci-logs")).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if !keep.contains(&entry.file_name().to_string_lossy().into_owned()) {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
    }
    Ok(items)
}

/// A job's log as "Read the failing log" shows it: less showed the file
/// with its colors, which a pane drawn as plain text can't, so the color
/// escapes go, as do the timestamp Actions puts first on every line and its
/// group marks; its error and warning marks are said in words.
pub fn readable(log: &str) -> String {
    static COLOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").unwrap());
    static STAMP: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^\d{4}-\d\d-\d\dT[\d:.]+Z ?").unwrap());
    let clean = COLOR.replace_all(log, "");
    let clean = STAMP.replace_all(&clean, "");
    clean
        .replace("\r\n", "\n")
        .replace("##[group]", "")
        .replace("##[endgroup]", "")
        .replace("##[error]", "Error: ")
        .replace("##[warning]", "Warning: ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_reads_without_its_colors_timestamps_and_marks() {
        let log = "2026-10-28T10:00:00.0000000Z ##[group]Run tests\n\
                   2026-10-28T10:00:01.0000000Z \x1b[31mFAIL\x1b[0m a test\n\
                   2026-10-28T10:00:02.0000000Z ##[endgroup]\n\
                   2026-10-28T10:00:03.0000000Z ##[error]Process completed with exit code 1.";
        assert_eq!(
            readable(log),
            "Run tests\nFAIL a test\n\nError: Process completed with exit code 1."
        );
    }

    #[test]
    fn a_failure_past_the_first_page_of_checks_is_found_once_the_rest_is_read() {
        let passing: Vec<Value> = (0..100)
            .map(|i| json!({ "__typename": "CheckRun", "name": format!("job {i}"), "conclusion": "SUCCESS" }))
            .collect();
        let mut pr = json!({ "number": 9, "repository": { "nameWithOwner": "o/r" },
            "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "FAILURE", "contexts": {
                "pageInfo": { "hasNextPage": true, "endCursor": "c1" }, "nodes": passing.clone() } } } }] } });
        assert!(failing_of(&pr).is_empty());
        assert!(needs_rest(&pr));
        let mut nodes = passing;
        nodes.push(json!({ "__typename": "CheckRun", "name": "e2e", "conclusion": "FAILURE", "detailsUrl": "https://github.com/o/r/actions/runs/1/job/12" }));
        with_checks(
            &mut pr,
            github::Paged {
                nodes,
                complete: true,
            },
        );
        let failing = failing_of(&pr);
        assert_eq!(failing.len(), 1);
        assert_eq!(failing[0].name, "e2e");
        assert_eq!(failing[0].job.as_deref(), Some("12"));
        assert!(!needs_rest(&pr), "found on the pages read");
    }

    #[test]
    fn a_pull_request_s_failing_checks_and_the_log_line_that_says_why() {
        // lib/sidebar's "the CI view finds a pull request's failing checks
        // and the log line that says why".
        let pr = json!({ "number": 9, "title": "Fix", "url": "https://github.com/o/r/pull/9",
            "headRefName": "fix", "repository": { "nameWithOwner": "o/r" },
            "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "contexts": { "nodes": [
            { "__typename": "CheckRun", "name": "lint", "conclusion": "SUCCESS", "detailsUrl": "https://github.com/o/r/actions/runs/1/job/10" },
            { "__typename": "CheckRun", "name": "test", "conclusion": "FAILURE", "detailsUrl": "https://github.com/o/r/actions/runs/1/job/11" },
            { "__typename": "StatusContext", "context": "deploy/preview", "state": "ERROR", "targetUrl": "https://ci.example/2" }
        ] } } } }] } });
        let failing = failing_of(&pr);
        assert_eq!(
            failing,
            vec![
                Failing {
                    name: "test".into(),
                    url: Some("https://github.com/o/r/actions/runs/1/job/11".into()),
                    job: Some("11".into())
                },
                Failing {
                    name: "deploy/preview".into(),
                    url: Some("https://ci.example/2".into()),
                    job: None
                },
            ]
        );
        let log = [
            "2026-10-28T10:00:00.0000000Z \x1b[36mRunning tests\x1b[0m",
            "2026-10-28T10:00:01.0000000Z some error happened in a dependency, ignored",
            "2026-10-28T10:00:02.0000000Z FAIL src/user.test.ts > creates a user",
            "2026-10-28T10:00:03.0000000Z  ELIFECYCLE  Command failed with exit code 1.",
            "2026-10-28T10:00:04.0000000Z ##[error]Process completed with exit code 1.",
        ]
        .join("\n");
        assert_eq!(
            why_from(&log),
            "FAIL src/user.test.ts > creates a user",
            "the tool's own failure, not the runner's"
        );
        assert_eq!(why_from("one\nsome error here\nlast"), "some error here");
        assert_eq!(why_from("nothing\nto see"), "to see");
        let item = item_of(&pr, &failing, Some("FAIL x")).unwrap();
        assert_eq!(item.title, "#9 Fix");
        assert_eq!(item.detail, "✗ test, deploy/preview · FAIL x");
        assert_eq!(item.data["ci"]["job"], "11");
        assert_eq!(item.data["ci"]["branch"], "fix");
    }
}
