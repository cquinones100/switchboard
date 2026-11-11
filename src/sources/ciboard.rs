//! Mission control's CI view, ported from lib/sidebar/ciboard.js: your open
//! pull requests in the repositories your tabs are in (and a fork's
//! upstream, as the PR board searches them), failing, running or passing,
//! each failing one with the checks that failed and, for each, the step of
//! its log that failed and the line that says why. One search for
//! the pull requests and their first hundred checks (and, for a PR with
//! more, the rest a hundred a query, through
//! `github::rest_of`), then the log of each failing GitHub Actions job,
//! read once (a finished job's log doesn't change) and kept in
//! `ci-logs/<job>.log` in the state directory, where the next look cuts its
//! failing step again (`failing_step`) and "Read the failing log" reads it;
//! at most `MAX_LOGS` are read a look, and the logs of jobs no longer
//! failing go, but those of the default branch's failing CI
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
const FIELDS: &str = "... on PullRequest { number title url headRefName repository { nameWithOwner } \
  commits(last: 1) { nodes { commit { statusCheckRollup { state contexts(first: 100) { \
    pageInfo { hasNextPage endCursor } nodes { \
    __typename ... on CheckRun { name status conclusion detailsUrl startedAt completedAt } ... on StatusContext { context state targetUrl } \
  } } } } } } }";
/// One PR's checks a page at a time, for a failing PR whose failures
/// GitHub didn't put on the search's first page.
const CHECKS_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!, $after: String) { \
  repository(owner: $owner, name: $name) { pullRequest(number: $number) { \
  commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100, after: $after) { \
    pageInfo { hasNextPage endCursor } nodes { \
    __typename ... on CheckRun { name status conclusion detailsUrl startedAt completedAt } ... on StatusContext { context state targetUrl } \
  } } } } } } } } }";
/// Where a PR's checks are, in the search's answer and in `CHECKS_QUERY`'s.
const CONTEXTS: &str = "/commits/nodes/0/commit/statusCheckRollup/contexts";
/// The most pages of a PR's checks read, a thousand checks.
const CHECK_PAGES: usize = 10;

/// A failing check: its name, its page, its GitHub Actions run's and
/// job's ids when it is one, and how long it ran, in seconds, when GitHub
/// says when it started and ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failing {
    pub name: String,
    pub url: Option<String>,
    pub job: Option<String>,
    pub run: Option<String>,
    pub secs: Option<i64>,
}

/// Where a check stands: failed, still running (queued, in progress, a
/// status pending), or done without failing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Failed,
    Running,
    Passed,
}

/// A check run's conclusion, else a status's state, as GitHub gives it.
fn outcome_of(c: &Value) -> Outcome {
    let said = c
        .get("conclusion")
        .filter(|v| !v.is_null())
        .or_else(|| c.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_uppercase();
    // Only success, neutral and skipped pass, as `wait-ci` counts them: a
    // canceled or stale check never passed, so it counts as failed.
    match said.as_str() {
        "" | "PENDING" | "EXPECTED" => Outcome::Running,
        "SUCCESS" | "NEUTRAL" | "SKIPPED" => Outcome::Passed,
        _ => Outcome::Failed,
    }
}

fn contexts(pr: &Value) -> impl Iterator<Item = &Value> {
    pr.pointer("/commits/nodes/0/commit/statusCheckRollup/contexts/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// How many of a PR's checks failed, are running and passed.
pub fn counts_of(pr: &Value) -> (usize, usize, usize) {
    contexts(pr).fold((0, 0, 0), |(f, r, p), c| match outcome_of(c) {
        Outcome::Failed => (f + 1, r, p),
        Outcome::Running => (f, r + 1, p),
        Outcome::Passed => (f, r, p + 1),
    })
}

/// Seconds between two of GitHub's times.
fn secs_between(start: &str, end: &str) -> Option<i64> {
    let start: jiff::Timestamp = start.parse().ok()?;
    let end: jiff::Timestamp = end.parse().ok()?;
    Some(end.as_second() - start.as_second())
}

/// The failing checks of a pull request.
pub fn failing_of(pr: &Value) -> Vec<Failing> {
    static JOB: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"/actions/runs/(\d+)/job/(\d+)").unwrap());
    contexts(pr)
        .filter(|c| outcome_of(c) == Outcome::Failed)
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
            let ids = url.as_deref().and_then(|u| JOB.captures(u));
            let run = ids.as_ref().map(|m| m[1].to_string());
            let job = ids.as_ref().map(|m| m[2].to_string());
            let secs = match (
                c.get("startedAt").and_then(Value::as_str),
                c.get("completedAt").and_then(Value::as_str),
            ) {
                (Some(start), Some(end)) => secs_between(start, end),
                _ => None,
            };
            Failing {
                name,
                url,
                job,
                run,
                secs,
            }
        })
        .collect()
}

/// The line of a log that says why it failed: the last that says so the way
/// tools do (FAIL, Error:, ✗, a traceback), else the last that mentions an
/// error or a failure, leaving out the lines that only say a command failed,
/// else its last line, without the timestamp Actions puts first.
pub fn why_from(log: &str) -> String {
    why_line(log).map(|(_, why)| why).unwrap_or_default()
}

/// The line `why_from` picks, with its place among the log's lines.
fn why_line(log: &str) -> Option<(usize, String)> {
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
    let lines: Vec<(usize, String)> = clean
        .split('\n')
        .map(|l| {
            MARK.replace_all(&STAMP.replace(l, ""), "")
                .trim()
                .to_string()
        })
        .enumerate()
        .filter(|(_, l)| !l.is_empty())
        .collect();
    let last = |re: &Regex| {
        lines
            .iter()
            .rev()
            .find(|(_, l)| re.is_match(l) && !NOISE.is_match(l))
            .cloned()
    };
    last(&STRONG)
        .or_else(|| last(&WEAK))
        .or_else(|| lines.last().cloned())
}

/// The most lines of a step kept for the pane beside the CI view.
const STEP_LINES: usize = 200;
/// The lines a log with no steps shows from its end.
const TAIL_LINES: usize = 40;

/// The step of a job's log that failed, as the CI view shows it beside
/// its list: the step's name, the number in the whole log of its first
/// line kept, its lines as `readable` cleans them, and the place among
/// them of the line that says why (`why_from`'s, within the step), with
/// that line's words.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Step {
    pub name: String,
    pub first: usize,
    pub lines: Vec<String>,
    pub why_at: Option<usize>,
    pub why: String,
}

impl Step {
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name, "first": self.first, "lines": self.lines,
            "whyAt": self.why_at, "why": self.why,
        })
    }
}

/// Cuts the failing step out of a job's log. Actions starts each step
/// with a `##[group]Run <command>` line, and its output runs on to the
/// next step's: the step is the one holding the last `##[error]`, else the
/// one holding the line `why_from` picks, else `TAIL_LINES` around that
/// line. A step longer than `STEP_LINES` keeps that many, the
/// why line among them.
pub fn failing_step(log: &str) -> Step {
    let raw: Vec<&str> = log.trim_end_matches('\n').split('\n').collect();
    let starts: Vec<usize> = raw
        .iter()
        .enumerate()
        .filter(|(_, l)| l.contains("##[group]Run "))
        .map(|(i, _)| i)
        .collect();
    let holding = |at: usize| -> Option<(usize, usize)> {
        let start = *starts.iter().rev().find(|s| **s <= at)?;
        let end = starts
            .iter()
            .find(|s| **s > at)
            .copied()
            .unwrap_or(raw.len());
        Some((start, end))
    };
    let error_at = raw.iter().rposition(|l| l.contains("##[error]"));
    let (start, end, name) = match error_at
        .and_then(holding)
        .or_else(|| why_line(log).and_then(|(at, _)| holding(at)))
    {
        Some((start, end)) => {
            let name = readable(raw[start]).trim().to_string();
            let name = name.strip_prefix("Run ").unwrap_or(&name).to_string();
            (start, end, name)
        }
        // No step holds it: the lines around the whole log's line that
        // says why, else its last.
        None => {
            let start = match why_line(log) {
                Some((at, _)) => at
                    .saturating_sub(TAIL_LINES * 3 / 4)
                    .min(raw.len().saturating_sub(TAIL_LINES)),
                None => raw.len().saturating_sub(TAIL_LINES),
            };
            (start, (start + TAIL_LINES).min(raw.len()), String::new())
        }
    };
    let section = raw[start..end].join("\n");
    let why = why_line(&section);
    let why_in = why.as_ref().map(|(at, _)| *at);
    let from = match why_in {
        Some(at) if end - start > STEP_LINES => at
            .saturating_sub(STEP_LINES * 3 / 4)
            .min(end - start - STEP_LINES),
        _ => 0,
    };
    let to = (from + STEP_LINES).min(end - start);
    Step {
        name,
        first: start + from + 1,
        lines: raw[start + from..start + to]
            .iter()
            .map(|l| readable(l).trim_end().to_string())
            .collect(),
        why_at: why_in
            .filter(|at| (from..to).contains(at))
            .map(|at| at - from),
        why: why.map(|(_, w)| w).unwrap_or_default(),
    }
}

/// Where a job's log is kept.
pub fn log_file(job: &str) -> PathBuf {
    paths::state_dir()
        .join("ci-logs")
        .join(format!("{job}.log"))
}

/// A failing PR's row: its checks that failed, each with the failing
/// step of its log when it was read (`steps`, beside `failing`), and why
/// the first read says it failed.
pub fn item_of(pr: &Value, failing: &[Failing], steps: &[Option<Step>]) -> Option<Item> {
    let number = pr.get("number").and_then(Value::as_u64)?;
    let url = text(pr, "/url");
    let repo = text(pr, "/repository/nameWithOwner");
    let names: Vec<&str> = failing.iter().map(|f| f.name.as_str()).collect();
    let mut detail = format!("✗ {}", names.join(", "));
    let step_of = |i: usize| steps.get(i).and_then(Option::as_ref);
    let logged = failing.iter().position(|f| f.job.is_some());
    let why = logged.and_then(step_of).map(|s| s.why.as_str());
    if let Some(why) = why.filter(|w| !w.is_empty()) {
        detail.push_str(" · ");
        detail.push_str(why);
    }
    let first = failing.first();
    let job = logged.map(|i| &failing[i]);
    let (job, job_name) = (job.and_then(|f| f.job.clone()), job.map(|f| f.name.clone()));
    let checks: Vec<Value> = failing
        .iter()
        .enumerate()
        .map(|(i, f)| {
            json!({
                "name": f.name, "url": f.url, "job": f.job, "run": f.run, "secs": f.secs,
                "why": step_of(i).map(|s| s.why.clone()),
                "step": step_of(i).map(Step::to_json),
            })
        })
        .collect();
    let (failed, running, passed) = counts_of(pr);
    Some(
        Item::new(url, format!("#{number} {}", text(pr, "/title")))
            .detail(detail)
            .state(State::Attention)
            .target(Target::Url(url.to_string()))
            .data(json!({
                "ci": {
                    "number": number, "repo": repo, "url": url,
                    "branch": text(pr, "/headRefName"),
                    "state": "failing",
                    "failed": failed, "running": running, "passed": passed,
                    "total": failed + running + passed,
                    "check": first.map(|f| json!({ "name": f.name, "url": f.url, "job": f.job })),
                    "checks": checks,
                    "job": job,
                    "logged": why.is_some(),
                    "jobName": job_name,
                },
                "search": repo,
            })),
    )
}

/// A PR with no failing check: running while any check is, else passing,
/// its detail its checks counted ("9/9", "4 of 5 passed, 1 running"); none
/// for a PR with no checks, whose CI says nothing yet.
pub fn progress_item(pr: &Value) -> Option<Item> {
    let number = pr.get("number").and_then(Value::as_u64)?;
    let url = text(pr, "/url");
    let repo = text(pr, "/repository/nameWithOwner");
    let (failed, running, passed) = counts_of(pr);
    let total = failed + running + passed;
    if total == 0 {
        return None;
    }
    let (state, detail) = if running > 0 {
        (
            "running",
            format!("{passed} of {total} passed, {running} running"),
        )
    } else {
        ("passing", format!("{passed}/{total}"))
    };
    Some(
        Item::new(url, format!("#{number} {}", text(pr, "/title")))
            .detail(detail)
            .state(if running > 0 {
                State::Active
            } else {
                State::Idle
            })
            .target(Target::Url(url.to_string()))
            .data(json!({
                "ci": {
                    "number": number, "repo": repo, "url": url,
                    "branch": text(pr, "/headRefName"),
                    "state": state,
                    "failed": failed, "running": running, "passed": passed, "total": total,
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

/// Whether a PR's checks past the search's first page are needed: there
/// are more, whose failures each get a line and whose running or passing
/// the counts take in.
fn needs_rest(pr: &Value) -> bool {
    pr.pointer(&format!("{CONTEXTS}/pageInfo/hasNextPage"))
        .and_then(Value::as_bool)
        == Some(true)
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
        contexts["pageInfo"]["hasNextPage"] = json!(!paged.complete);
    }
}

/// The order the failing PRs' Actions jobs are read in, each as its PR's
/// place and the job's among the PR's failed checks: every PR's first
/// job, then every PR's second, and so on, so a status of another CI or
/// logs that can't be read on one PR never put another PR's first late.
fn read_order(prs: &[&[Failing]]) -> Vec<(usize, usize)> {
    let jobs: Vec<Vec<usize>> = prs
        .iter()
        .map(|failing| {
            (0..failing.len())
                .filter(|&i| failing[i].job.is_some())
                .collect()
        })
        .collect();
    let most = jobs.iter().map(Vec::len).max().unwrap_or(0);
    (0..most)
        .flat_map(|round| {
            jobs.iter()
                .enumerate()
                .filter_map(move |(p, j)| j.get(round).map(|&nth| (p, nth)))
        })
        .collect()
}

/// Looks for your open PRs in the repositories and where their checks
/// stand: the failing ones first, with why each failed check fails, then
/// those running, then those passing. A failed job's log asked of GitHub
/// counts against the look's `MAX_LOGS`; one already kept doesn't, so a
/// later look reads those it had no room for.
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
    let mut others = Vec::new();
    let mut failing_prs = Vec::new();
    for pr in prs.iter().filter(|p| p.get("number").is_some()) {
        let failing = failing_of(pr);
        if failing.is_empty() {
            others.extend(progress_item(pr));
            continue;
        }
        let steps = vec![None; failing.len()];
        failing_prs.push((pr, failing, steps));
    }
    // Each PR's first failed job is read before any PR's second, so logs
    // that can't be read (expired, say) on one PR never use up the look's
    // `MAX_LOGS` before every PR has had its first. A kept log is read from
    // its file, so only one asked of GitHub counts against it.
    let order = {
        let failing: Vec<&[Failing]> = failing_prs.iter().map(|(_, f, _)| f.as_slice()).collect();
        read_order(&failing)
    };
    let mut logs = 0;
    for (p, nth) in order {
        let (pr, failing, steps) = &mut failing_prs[p];
        let Some(job) = failing[nth].job.as_deref() else {
            continue;
        };
        let kept = log_file(job).is_file();
        if !kept && logs >= MAX_LOGS {
            continue;
        }
        logs += usize::from(!kept);
        let repo = text(pr, "/repository/nameWithOwner");
        steps[nth] = read_log(repo, job).await.map(|log| failing_step(&log));
    }
    let mut items: Vec<Item> = failing_prs
        .iter()
        .filter_map(|(pr, failing, steps)| item_of(pr, failing, steps))
        .collect();
    // Running before passing, each in the search's order.
    others.sort_by_key(|i| i.data.pointer("/ci/state") != Some(&json!("running")));
    items.extend(others);
    // Logs of jobs no longer failing go.
    let keep: Vec<String> = items
        .iter()
        .flat_map(|i| {
            i.data
                .pointer("/ci/checks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|c| c["job"].as_str())
        })
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
            { "__typename": "CheckRun", "name": "test", "conclusion": "FAILURE", "detailsUrl": "https://github.com/o/r/actions/runs/1/job/11",
              "startedAt": "2026-10-28T10:00:00Z", "completedAt": "2026-10-28T10:04:12Z" },
            { "__typename": "StatusContext", "context": "deploy/preview", "state": "ERROR", "targetUrl": "https://ci.example/2" }
        ] } } } }] } });
        let failing = failing_of(&pr);
        assert_eq!(
            failing,
            vec![
                Failing {
                    name: "test".into(),
                    url: Some("https://github.com/o/r/actions/runs/1/job/11".into()),
                    job: Some("11".into()),
                    run: Some("1".into()),
                    secs: Some(252),
                },
                Failing {
                    name: "deploy/preview".into(),
                    url: Some("https://ci.example/2".into()),
                    job: None,
                    run: None,
                    secs: None,
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
        let step = Step {
            why: "FAIL x".into(),
            ..Step::default()
        };
        let item = item_of(&pr, &failing, &[Some(step), None]).unwrap();
        assert_eq!(item.title, "#9 Fix");
        assert_eq!(item.detail, "✗ test, deploy/preview · FAIL x");
        assert_eq!(item.data["ci"]["job"], "11");
        assert_eq!(item.data["ci"]["branch"], "fix");
        assert_eq!(item.data["ci"]["state"], "failing");
        assert_eq!(item.data["ci"]["failed"], 2);
        assert_eq!(item.data["ci"]["total"], 3);
        let checks = item.data["ci"]["checks"].as_array().unwrap();
        assert_eq!(checks[0]["why"], "FAIL x");
        assert_eq!(checks[0]["run"], "1");
        assert_eq!(checks[0]["secs"], 252);
        assert_eq!(checks[1]["name"], "deploy/preview");
        assert_eq!(checks[1]["why"], Value::Null, "no log to read");
    }

    fn pr_with(checks: Value) -> Value {
        json!({ "number": 7, "title": "Image cache", "url": "https://github.com/o/r/pull/7",
            "headRefName": "cache", "repository": { "nameWithOwner": "o/r" },
            "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "contexts": { "nodes": checks } } } }] } })
    }

    #[test]
    fn a_running_and_a_passing_pr_get_their_state_and_counts() {
        let run = |name: &str, status: &str, conclusion: Value| json!({ "__typename": "CheckRun", "name": name, "status": status, "conclusion": conclusion });
        let running = pr_with(json!([
            run("lint", "COMPLETED", json!("SUCCESS")),
            run("test", "IN_PROGRESS", Value::Null),
            { "__typename": "StatusContext", "context": "deploy", "state": "PENDING" },
            run("docs", "COMPLETED", json!("SKIPPED")),
        ]));
        assert!(failing_of(&running).is_empty());
        let item = progress_item(&running).unwrap();
        assert_eq!(item.data["ci"]["state"], "running");
        assert_eq!(item.detail, "2 of 4 passed, 2 running");
        assert_eq!(item.state, State::Active);
        let passing = pr_with(json!([
            run("lint", "COMPLETED", json!("SUCCESS")),
            run("test", "COMPLETED", json!("SUCCESS")),
        ]));
        let item = progress_item(&passing).unwrap();
        assert_eq!(item.data["ci"]["state"], "passing");
        assert_eq!(item.detail, "2/2");
        assert_eq!(counts_of(&passing), (0, 0, 2));
        assert!(
            progress_item(&pr_with(json!([]))).is_none(),
            "no checks, no CI to show"
        );
        let canceled = pr_with(json!([
            run("lint", "COMPLETED", json!("SUCCESS")),
            run("test", "COMPLETED", json!("CANCELLED")),
        ]));
        assert_eq!(
            failing_of(&canceled).len(),
            1,
            "a canceled check never passed"
        );
        let stale = pr_with(json!([run("test", "COMPLETED", json!("STALE"))]));
        assert_eq!(failing_of(&stale).len(), 1, "nor did a stale one");
    }

    #[test]
    fn the_failing_step_is_the_one_holding_the_last_error_numbered_as_in_the_log() {
        let log = [
            "2026-10-28T10:00:00.0Z ##[group]Run actions/checkout@v4",
            "2026-10-28T10:00:01.0Z ##[endgroup]",
            "2026-10-28T10:00:02.0Z checked out",
            "2026-10-28T10:00:03.0Z ##[group]Run cargo test",
            "2026-10-28T10:00:04.0Z shell: /usr/bin/bash -e {0}",
            "2026-10-28T10:00:05.0Z ##[endgroup]",
            "2026-10-28T10:00:06.0Z running 3 tests",
            "2026-10-28T10:00:07.0Z thread 'x' panicked at src/a.rs:3:5:",
            "2026-10-28T10:00:08.0Z test result: FAILED. 2 passed; 1 failed",
            "2026-10-28T10:00:09.0Z ##[error]Process completed with exit code 101.",
            "2026-10-28T10:00:10.0Z ##[group]Run actions/cache/save@v4",
            "2026-10-28T10:00:11.0Z ##[endgroup]",
        ]
        .join("\n");
        let step = failing_step(&log);
        assert_eq!(step.name, "cargo test");
        assert_eq!(
            step.first, 4,
            "the step's first line, numbered in the whole log"
        );
        assert_eq!(step.lines.len(), 7);
        assert_eq!(step.lines[0], "Run cargo test");
        assert_eq!(step.why, "test result: FAILED. 2 passed; 1 failed");
        assert_eq!(step.why_at, Some(5));
        assert_eq!(
            step.lines[5], "test result: FAILED. 2 passed; 1 failed",
            "cleaned as the whole log is"
        );
        assert_eq!(
            step.lines[6],
            "Error: Process completed with exit code 101."
        );
    }

    #[test]
    fn a_log_with_no_steps_shows_its_last_lines() {
        let log: Vec<String> = (1..=60).map(|i| format!("line {i}")).collect();
        let mut log = log.join("\n");
        log.push_str("\nFAIL the end");
        let step = failing_step(&log);
        assert_eq!(step.name, "");
        assert_eq!(step.lines.len(), 40);
        assert_eq!(step.first, 22);
        assert_eq!(step.why, "FAIL the end");
        assert_eq!(step.why_at, Some(39));
    }

    #[test]
    fn a_log_failing_before_any_step_keeps_its_error_among_the_lines_shown() {
        let mut log = vec!["##[error]Error: setup failed".to_string()];
        log.extend((0..41).map(|i| format!("cleanup {i}")));
        let step = failing_step(&log.join("\n"));
        assert_eq!(step.why, "Error: setup failed");
        assert_eq!(step.first, 1);
        assert_eq!(step.why_at, Some(0));
        assert_eq!(step.lines.len(), TAIL_LINES);
    }

    #[test]
    fn each_pr_s_first_actions_job_is_read_before_any_pr_s_second() {
        let check = |job: Option<&str>| Failing {
            name: "c".into(),
            url: None,
            job: job.map(str::to_string),
            run: None,
            secs: None,
        };
        let a = [check(Some("1")), check(Some("2"))];
        let b = [check(None), check(Some("3"))];
        assert_eq!(
            read_order(&[&a, &b]),
            vec![(0, 0), (1, 1), (0, 1)],
            "another CI's status first doesn't put b's first job late"
        );
    }

    #[test]
    fn a_long_step_keeps_its_why_line_among_the_lines_kept() {
        let mut log = vec!["##[group]Run make".to_string()];
        log.extend((0..500).map(|i| format!("ok {i}")));
        log.push("Error: broke".into());
        log.extend((0..300).map(|i| format!("after {i}")));
        let step = failing_step(&log.join("\n"));
        assert_eq!(step.lines.len(), STEP_LINES);
        let at = step.why_at.expect("kept");
        assert_eq!(step.lines[at], "Error: broke");
        assert_eq!(step.first + at, 502, "its number in the whole log");
    }
}
