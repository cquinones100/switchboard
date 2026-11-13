//! Whether the branch's PR is ready to merge (M-m; tmux/pr-ready.sh): a
//! verdict first, then each thing that decides it, in a box over the
//! workspace, ending with what can be done about it (see the failing
//! checks, or have the tab's Claude resolve the conflicts).
//!
//! Asked in the order that can end the answer soonest. A merged or closed
//! PR says so and stops. A PR already in the merge queue reports its place
//! there and stops too: its checks, conflicts and review were settled by
//! getting in, and GitHub still calls it mergeable, so the rest would only
//! say "ready" about something already on its way. For anything else the
//! verdict is GitHub's own mergeStateStatus, so it agrees with the merge
//! button, with the lines under it explaining it. Checks are split into
//! required ones and all of them, since a failing optional check (a preview
//! deploy) doesn't block the merge and shouldn't read as if it did.
//!
//! Where pr-ready.sh asked gh three times (the PR, its queue entry and
//! checks counted by outcome, and `gh pr checks --required` one by one),
//! one GraphQL query gives it all here: the counts by state, and each of
//! the first hundred checks with whether it is required, and the rest a
//! hundred a query (`github::rest_of`, up to a thousand) only when a check
//! failed or is running, since a failure can be anywhere in GitHub's list.
//! GitHub works out mergeability lazily, so an UNKNOWN is asked again, up
//! to three times a second apart, as pr-ready.sh did.
//!
//! A branch with no PR (one pushed before opening it, the default branch
//! of a repository that lands straight on it) gets the CI of the commit
//! GitHub has for it instead, from a second query on the branch's ref where
//! git pushes it (`BRANCH_QUERY`), with no conflicts or review lines, which
//! only a PR has. The commit is GitHub's, so the answer says when this tab's
//! `HEAD` isn't it: ahead of it or behind it by some commits, or elsewhere.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::LazyLock;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use regex::Regex;
use serde_json::{Value, json};

use super::pager::{self, Content};
use crate::integrations::github;
use crate::mux::input::{Key, KeyCode, Mods, Mouse};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use crate::ui::menu::{Filter, Filtered, NO_MATCH, SEARCH_KEYS};
use crate::ui::{Look, Outcome, Overlay, Step};
use crate::util;

const SOURCE: &str = "M-m PR check";

const QUERY: &str = "query($owner: String!, $name: String!, $number: Int!, $after: String) { repository(owner: $owner, name: $name) { \
    pullRequest(number: $number) { number title url state isDraft mergeable mergeStateStatus reviewDecision \
    autoMergeRequest { enabledAt } mergeQueueEntry { state position } \
    commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100, after: $after) { \
    checkRunCountsByState { state count } statusContextCountsByState { state count } \
    pageInfo { hasNextPage endCursor } \
    nodes { __typename \
      ... on CheckRun { name status conclusion detailsUrl isRequired(pullRequestNumber: $number) } \
      ... on StatusContext { context state targetUrl description isRequired(pullRequestNumber: $number) } \
    } } } } } } } } }";

const BRANCH_QUERY: &str = "query($owner: String!, $name: String!, $qualified: String!, $after: String) { repository(owner: $owner, name: $name) { \
    ref(qualifiedName: $qualified) { target { ... on Commit { oid statusCheckRollup { contexts(first: 100, after: $after) { \
    checkRunCountsByState { state count } statusContextCountsByState { state count } \
    pageInfo { hasNextPage endCursor } \
    nodes { __typename \
      ... on CheckRun { name status conclusion detailsUrl } \
      ... on StatusContext { context state targetUrl description } \
    } } } } } } } }";

/// What the answer offers to do, by the number the box gives back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    SeeFailing = 0,
    ResolveConflicts = 1,
    Close = 2,
}

impl Choice {
    pub fn from_index(i: usize) -> Choice {
        match i {
            0 => Choice::SeeFailing,
            1 => Choice::ResolveConflicts,
            _ => Choice::Close,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Choice::SeeFailing => "See the failing checks",
            Choice::ResolveConflicts => "Type /resolve-conflicts into this tab's Claude",
            Choice::Close => "Close",
        }
    }
}

/// The answer, as the box shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub lines: Vec<Line<'static>>,
    pub choices: Vec<Choice>,
}

impl Answer {
    /// Its text, for the tests.
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn green() -> Style {
    Style::new().fg(Color::Green)
}
fn red() -> Style {
    Style::new().fg(Color::Red)
}
fn yellow() -> Style {
    Style::new().fg(Color::Yellow)
}
fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}
fn bold(style: Style) -> Style {
    style.add_modifier(Modifier::BOLD)
}

/// A check's state in gh's buckets, as `gh pr checks` says them.
fn bucket(state: &str) -> &'static str {
    match state {
        "SUCCESS" | "NEUTRAL" | "COMPLETED" => "pass",
        "SKIPPED" => "skipping",
        "CANCELLED" => "cancel",
        "IN_PROGRESS" | "PENDING" | "QUEUED" | "WAITING" | "EXPECTED" => "pending",
        _ => "fail",
    }
}

/// One check's bucket: a check run by its conclusion once it has
/// completed, a status by its state.
fn check_bucket(node: &Value) -> &'static str {
    let text = |k: &str| node.get(k).and_then(Value::as_str).unwrap_or("");
    if node.get("__typename").and_then(Value::as_str) == Some("CheckRun") {
        if text("status") != "COMPLETED" {
            return "pending";
        }
        return bucket(text("conclusion"));
    }
    bucket(text("state"))
}

/// Counts by bucket, `3 pass, 1 fail`, in gh's order; "none" for none.
fn counts(pairs: impl Iterator<Item = (&'static str, u64)>) -> String {
    let mut by: Vec<(&str, u64)> = Vec::new();
    for (b, n) in pairs.filter(|(_, n)| *n > 0) {
        match by.iter_mut().find(|(k, _)| *k == b) {
            Some((_, total)) => *total += n,
            None => by.push((b, n)),
        }
    }
    let order = ["pass", "fail", "pending", "cancel", "skipping"];
    by.sort_by_key(|(b, _)| order.iter().position(|o| o == b).unwrap_or(9));
    if by.is_empty() {
        return "none".into();
    }
    by.iter()
        .map(|(b, n)| format!("{n} {b}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The answer from what GitHub said of the PR.
pub fn answer_from(pr: &Value) -> Answer {
    let text = |k: &str| pr.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut lines = vec![
        Line::from(Span::styled(
            format!(
                "#{} {}",
                pr.get("number").map(util::js_string).unwrap_or_default(),
                text("title")
            ),
            bold(Style::new()),
        )),
        Line::from(Span::styled(text("url"), dim())),
        Line::default(),
    ];
    let said = |lines: &mut Vec<Line<'static>>, parts: Vec<(String, Style)>| {
        lines.push(Line::from(
            parts
                .into_iter()
                .map(|(t, s)| Span::styled(t, s))
                .collect::<Vec<_>>(),
        ));
    };
    match text("state").as_str() {
        "MERGED" => {
            said(&mut lines, vec![("Already merged.".into(), bold(green()))]);
            return Answer {
                lines,
                choices: Vec::new(),
            };
        }
        "CLOSED" => {
            said(
                &mut lines,
                vec![("Closed without merging.".into(), bold(red()))],
            );
            return Answer {
                lines,
                choices: Vec::new(),
            };
        }
        _ => {}
    }
    if let Some(entry) = pr.get("mergeQueueEntry").filter(|e| e.is_object()) {
        let state = entry.get("state").and_then(Value::as_str).unwrap_or("");
        let position = entry
            .get("position")
            .map(util::js_string)
            .unwrap_or_default();
        let at = format!(" at position {position}");
        let verdict = match state {
            "AWAITING_CHECKS" => vec![
                ("In the merge queue".into(), bold(green())),
                (format!("{at}, waiting on the queue's checks."), green()),
            ],
            "MERGEABLE" => vec![
                ("In the merge queue".into(), bold(green())),
                (format!("{at}, passed and waiting its turn."), green()),
            ],
            "LOCKED" => vec![
                ("In the merge queue".into(), bold(green())),
                (format!("{at}, merging now."), green()),
            ],
            "UNMERGEABLE" => vec![
                (
                    format!("In the merge queue{at}, but it cannot merge"),
                    bold(red()),
                ),
                (": it will be removed unless that changes.".into(), red()),
            ],
            other => vec![
                ("In the merge queue".into(), bold(green())),
                (
                    format!("{at} ({}).", other.to_lowercase().replace('_', " ")),
                    green(),
                ),
            ],
        };
        said(&mut lines, verdict);
        return Answer {
            lines,
            choices: Vec::new(),
        };
    }
    let merge_state = text("mergeStateStatus");
    let verdict: Vec<(String, Style)> = match merge_state.as_str() {
        "CLEAN" => vec![("Ready to merge.".into(), bold(green()))],
        "HAS_HOOKS" => vec![
            ("Ready to merge".into(), bold(green())),
            (" (the repo runs merge hooks).".into(), green()),
        ],
        "UNSTABLE" => vec![
            ("Ready to merge".into(), bold(green())),
            (
                ", though some optional checks are not passing.".into(),
                yellow(),
            ),
        ],
        "BEHIND" => vec![
            ("Not ready:".into(), bold(yellow())),
            (
                " the branch is behind its base and must be updated.".into(),
                yellow(),
            ),
        ],
        "BLOCKED" => vec![
            ("Not ready:".into(), bold(red())),
            (" blocked by a required review or check.".into(), red()),
        ],
        "DIRTY" => vec![
            ("Not ready:".into(), bold(red())),
            (" it has merge conflicts.".into(), red()),
        ],
        "DRAFT" => vec![
            ("Not ready:".into(), bold(yellow())),
            (" it is still a draft.".into(), yellow()),
        ],
        _ => vec![
            (
                "GitHub has not worked out whether it can merge yet.".into(),
                bold(yellow()),
            ),
            (" Try again in a moment.".into(), dim()),
        ],
    };
    said(&mut lines, verdict);
    lines.push(Line::default());
    let contexts = pr
        .pointer("/commits/nodes/0/commit/statusCheckRollup/contexts")
        .cloned()
        .unwrap_or(Value::Null);
    let state_counts = |key: &str| -> Vec<(&'static str, u64)> {
        contexts
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|c| {
                (
                    bucket(c.get("state").and_then(Value::as_str).unwrap_or("")),
                    c.get("count").and_then(Value::as_u64).unwrap_or(0),
                )
            })
            .collect()
    };
    let all = counts(
        state_counts("checkRunCountsByState")
            .into_iter()
            .chain(state_counts("statusContextCountsByState")),
    );
    let nodes: Vec<Value> = contexts
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let required: (String, Style) =
        if matches!(merge_state.as_str(), "CLEAN" | "HAS_HOOKS" | "UNSTABLE") {
            ("all passing".into(), green())
        } else {
            let shown = counts(
                nodes
                    .iter()
                    .filter(|n| n.get("isRequired").and_then(Value::as_bool) == Some(true))
                    .map(|n| (check_bucket(n), 1)),
            );
            (shown, Style::new())
        };
    let review = match text("reviewDecision").as_str() {
        "APPROVED" => ("approved".to_string(), green()),
        "CHANGES_REQUESTED" => ("changes requested".into(), red()),
        "REVIEW_REQUIRED" => ("review required".into(), yellow()),
        _ => ("none required".into(), dim()),
    };
    let mergeable = text("mergeable");
    let conflicts = match mergeable.as_str() {
        "MERGEABLE" => ("none".to_string(), green()),
        "CONFLICTING" => ("conflicts with the base".into(), red()),
        _ => ("unknown".into(), dim()),
    };
    let draft = if pr.get("isDraft").and_then(Value::as_bool) == Some(true) {
        ("yes".to_string(), yellow())
    } else {
        ("no".into(), Style::new())
    };
    let auto = if pr.get("autoMergeRequest").is_some_and(|a| !a.is_null()) {
        ("on".to_string(), green())
    } else {
        ("off".into(), Style::new())
    };
    for (label, (value, style)) in [
        ("Required checks", required),
        ("All checks", (all.clone(), Style::new())),
        ("Review", review),
        ("Conflicts", conflicts),
        ("Draft", draft),
        ("Auto-merge", auto),
    ] {
        said(
            &mut lines,
            vec![(format!("{label:<17} "), Style::new()), (value, style)],
        );
    }
    let mut choices = Vec::new();
    if all.contains("fail") {
        choices.push(Choice::SeeFailing);
    }
    if mergeable == "CONFLICTING" || merge_state == "DIRTY" {
        choices.push(Choice::ResolveConflicts);
    }
    Answer { lines, choices }
}

/// Where this tab's `HEAD` is against the commit GitHub has for the branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Local {
    /// The same commit.
    Checked,
    /// Ahead of it by this many commits.
    Ahead(u64),
    /// Behind it by this many commits.
    Behind(u64),
    /// Beside it, or nowhere git can tell.
    Elsewhere,
}

/// The counts of `contexts` (a `statusCheckRollup`'s) by gh's bucket.
fn bucket_counts(contexts: &Value) -> Vec<(&'static str, u64)> {
    ["checkRunCountsByState", "statusContextCountsByState"]
        .iter()
        .filter_map(|k| contexts.get(*k).and_then(Value::as_array))
        .flatten()
        .map(|c| {
            (
                bucket(c.get("state").and_then(Value::as_str).unwrap_or("")),
                c.get("count").and_then(Value::as_u64).unwrap_or(0),
            )
        })
        .collect()
}

/// The answer for a branch with no PR, from the commit GitHub has for it
/// (`checks` as `commit_checks` shapes it; `None` when GitHub has no such
/// branch) and where this tab is against it.
pub fn answer_from_branch(branch: &str, checks: Option<&Value>, local: Local) -> Answer {
    let Some(checks) = checks else {
        return Answer {
            lines: vec![Line::from(Span::styled(
                format!("No PR, and {branch} isn't on GitHub."),
                bold(yellow()),
            ))],
            choices: Vec::new(),
        };
    };
    let oid = checks
        .pointer("/commits/nodes/0/commit/oid")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut lines = vec![
        Line::from(Span::styled(
            format!("No PR · CI on {branch} at {}", short(oid)),
            bold(Style::new()),
        )),
        Line::default(),
    ];
    let contexts = checks.pointer(CONTEXTS).cloned().unwrap_or(Value::Null);
    let by_bucket = bucket_counts(&contexts);
    let total: u64 = by_bucket.iter().map(|(_, n)| n).sum();
    let of = |b: &str| -> u64 {
        by_bucket
            .iter()
            .filter(|(k, _)| *k == b)
            .map(|(_, n)| n)
            .sum()
    };
    let (failing, running, canceled) = (of("fail"), of("pending"), of("cancel"));
    let verdict = if total == 0 {
        Span::styled("No checks.", bold(dim()))
    } else if failing > 0 {
        Span::styled(format!("Failing: {failing} of {total}."), bold(red()))
    } else if running > 0 {
        Span::styled(format!("Running: {running} of {total}."), bold(yellow()))
    } else if canceled > 0 {
        Span::styled(format!("Canceled: {canceled} of {total}."), bold(yellow()))
    } else {
        Span::styled("Passing.", bold(green()))
    };
    lines.push(Line::from(verdict));
    if total > 0 {
        lines.push(Line::from(vec![
            Span::raw(format!("{:<17} ", "All checks")),
            Span::raw(counts(by_bucket.into_iter())),
        ]));
    }
    let behind = match local {
        Local::Checked => None,
        Local::Ahead(1) => Some("This tab is 1 commit ahead of what GitHub checked.".to_string()),
        Local::Ahead(n) => Some(format!(
            "This tab is {n} commits ahead of what GitHub checked."
        )),
        Local::Behind(1) => Some("This tab is 1 commit behind what GitHub checked.".to_string()),
        Local::Behind(n) => Some(format!(
            "This tab is {n} commits behind what GitHub checked."
        )),
        Local::Elsewhere => Some("This tab's commit isn't the one GitHub checked.".into()),
    };
    if let Some(behind) = behind {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(behind, yellow())));
    }
    let choices = if failing > 0 {
        vec![Choice::SeeFailing]
    } else {
        Vec::new()
    };
    Answer { lines, choices }
}

/// A commit's first seven characters, as GitHub shows it.
fn short(oid: &str) -> &str {
    &oid[..oid.len().min(7)]
}

/// The commit `BRANCH_QUERY` found in the shape of a PR's answer, its checks
/// at `CONTEXTS`, so the paging and the failing checks read either alike.
fn commit_checks(commit: Value) -> Value {
    json!({ "commits": { "nodes": [{ "commit": commit }] } })
}

/// Where the PR's checks are in M-m's answer, from the PR and from `data`.
const CONTEXTS: &str = "/commits/nodes/0/commit/statusCheckRollup/contexts";
/// The most pages of checks read, a thousand checks.
const CHECK_PAGES: usize = 10;

/// Whether the checks GitHub left off the first page are needed: its
/// counts say a check failed or is still running, so the list of them has
/// to have every check, and there are more pages. A PR whose checks all
/// passed, or that has a hundred or fewer, costs one query.
fn needs_rest(pr: &Value) -> bool {
    let Some(contexts) = pr.pointer(CONTEXTS) else {
        return false;
    };
    let unsettled = bucket_counts(contexts)
        .iter()
        .any(|(b, n)| *n > 0 && matches!(*b, "fail" | "pending"));
    let more = contexts
        .pointer("/pageInfo/hasNextPage")
        .and_then(Value::as_bool)
        == Some(true);
    unsettled && more
}

/// The PR with every check read in place of the first page's.
fn with_checks(mut pr: Value, paged: github::Paged) -> Value {
    if let Some(contexts) = pr.pointer_mut(CONTEXTS) {
        contexts["nodes"] = Value::Array(paged.nodes);
    }
    pr
}

/// What M-m reads for a tab: its branch's PR, else the branch's own CI.
enum State {
    /// The PR as `QUERY` gives it, and whether every check is there.
    Pr {
        repo: github::Repo,
        pr: Value,
        complete: bool,
    },
    /// The branch's commit on GitHub (`commit_checks`'s shape; `None` when
    /// GitHub has no such branch), whether every check is there, and where
    /// this tab is against it.
    Branch {
        repo: github::Repo,
        branch: String,
        checks: Option<Value>,
        complete: bool,
        local: Local,
    },
}

/// The PR for `dir`'s branch as M-m's query gives it, asked again while
/// GitHub hasn't worked out whether it can merge, else the branch's own
/// checks; either with every check when any failed or is running
/// (`needs_rest`), and whether every check is there: false when there were
/// more than `CHECK_PAGES` pages.
async fn state_of(dir: &Path) -> Result<State, String> {
    let found = github::branch_pr(dir, SOURCE)
        .await
        .map_err(|e| e.to_string())?;
    let Some((repo, pr)) = found else {
        return branch_state(dir).await;
    };
    let variables = json!({ "owner": repo.owner, "name": repo.name, "number": pr.number });
    let mut answer = Value::Null;
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let data = github::graphql(QUERY, variables.clone(), SOURCE)
            .await
            .map_err(|e| e.to_string())?;
        answer = data
            .pointer("/repository/pullRequest")
            .cloned()
            .unwrap_or(Value::Null);
        if answer.get("mergeable").and_then(Value::as_str) != Some("UNKNOWN") {
            break;
        }
    }
    let (pr, complete) = every_check(answer, QUERY, &variables, "/repository/pullRequest").await?;
    Ok(State::Pr { repo, pr, complete })
}

/// `state_of` for a branch with no PR: the checks of the commit GitHub has
/// for it, in the repository it is pushed to.
async fn branch_state(dir: &Path) -> Result<State, String> {
    let pushed = github::pushed_branch(dir)
        .await
        .map_err(|e| e.to_string())?;
    let Some((repo, branch)) = pushed else {
        return Err("No pull request for this branch.".into());
    };
    let variables = json!({
        "owner": repo.owner, "name": repo.name,
        "qualified": format!("refs/heads/{branch}"),
    });
    let data = github::graphql(BRANCH_QUERY, variables.clone(), SOURCE)
        .await
        .map_err(|e| e.to_string())?;
    let Some(commit) = data
        .pointer("/repository/ref/target")
        .filter(|t| t.get("oid").is_some())
        .cloned()
    else {
        return Ok(State::Branch {
            repo,
            branch,
            checks: None,
            complete: true,
            local: Local::Checked,
        });
    };
    let oid = commit
        .get("oid")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let (checks, complete) = every_check(
        commit_checks(commit),
        BRANCH_QUERY,
        &variables,
        "/repository/ref/target",
    )
    .await?;
    let local = local_against(dir, &oid).await;
    Ok(State::Branch {
        repo,
        branch,
        checks: Some(checks),
        complete,
        local,
    })
}

/// `answer` (in a PR's shape) with every check read when `needs_rest` says
/// they are needed, `query` asked again for the pages after the first, its
/// checks at `at` in the query's data; and whether every check is there.
async fn every_check(
    answer: Value,
    query: &str,
    variables: &Value,
    at: &str,
) -> Result<(Value, bool), String> {
    if !needs_rest(&answer) {
        return Ok((answer, true));
    }
    let first = answer.pointer(CONTEXTS).cloned().unwrap_or(Value::Null);
    // A branch's commit is at the query's `target`, its checks right under
    // it, where a PR's are under its last commit.
    let contexts = if at.ends_with("/target") {
        format!("{at}/statusCheckRollup/contexts")
    } else {
        format!("{at}{CONTEXTS}")
    };
    let paged = github::rest_of(query, variables, &contexts, &first, CHECK_PAGES, SOURCE)
        .await
        .map_err(|e| e.to_string())?;
    let complete = paged.complete;
    Ok((with_checks(answer, paged), complete))
}

/// Where this tab's `HEAD` is against `oid`, the commit GitHub has for the
/// branch: the commit from git's files, and only when it differs, git asked
/// how far apart they are.
async fn local_against(dir: &Path, oid: &str) -> Local {
    let head = crate::tabs::git::head_commit(dir)
        .and_then(|h| h.split_whitespace().nth(1).map(str::to_string));
    if head.as_deref() == Some(oid) {
        return Local::Checked;
    }
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(["--no-optional-locks", "rev-list", "--left-right", "--count"])
        .arg(format!("{oid}...HEAD"));
    let Ok(output) = util::run_async(&mut cmd, Some(Duration::from_secs(10))).await else {
        return Local::Elsewhere;
    };
    if !output.status.success() {
        return Local::Elsewhere;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.split_whitespace().map(|n| n.parse::<u64>().ok());
    match (parts.next().flatten(), parts.next().flatten()) {
        (Some(0), Some(0)) => Local::Checked,
        (Some(0), Some(ahead)) => Local::Ahead(ahead),
        (Some(behind), Some(0)) => Local::Behind(behind),
        _ => Local::Elsewhere,
    }
}

/// M-m's answer for `dir`, or why there is none.
pub async fn check(dir: PathBuf) -> Result<Answer, String> {
    let (pr, complete) = match state_of(&dir).await? {
        State::Pr { pr, complete, .. } => (pr, complete),
        // GitHub's counts are whole, and a branch has no required checks to
        // count from the list.
        State::Branch {
            branch,
            checks,
            local,
            ..
        } => return Ok(answer_from_branch(&branch, checks.as_ref(), local)),
    };
    let mut answer = answer_from(&pr);
    if !complete {
        answer.lines.push(Line::from(Span::styled(
            "The counts of required checks leave out the ones GitHub didn't send.",
            dim(),
        )));
    }
    Ok(answer)
}

static JOB: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/actions/runs/\d+/job/(\d+)").unwrap());
static STAMP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{4}-\d\d-\d\dT[0-9:.]+Z ").unwrap());

/// The failing checks as Markdown: each with its description and link, and
/// a GitHub Actions job's last 25 lines of log, as pr-ready.sh's
/// `failing` showed them in less.
pub async fn failing_markdown(dir: &Path) -> Result<String, String> {
    Ok(match state_of(dir).await? {
        State::Pr { repo, pr, complete } => {
            let of = format!(
                "#{}",
                pr.get("number").map(util::js_string).unwrap_or_default()
            );
            failing_text(&repo, &of, &pr, complete).await
        }
        State::Branch {
            repo,
            branch,
            checks: Some(checks),
            complete,
            ..
        } => {
            let oid = checks
                .pointer("/commits/nodes/0/commit/oid")
                .and_then(Value::as_str)
                .unwrap_or("");
            let of = format!("`{branch}` at `{}`", short(oid));
            failing_text(&repo, &of, &checks, complete).await
        }
        State::Branch { branch, .. } => {
            return Err(format!("No PR, and {branch} isn't on GitHub."));
        }
    })
}

/// `failing_markdown`'s text for the checks as `state_of` read them (a PR's,
/// or a branch's commit in its shape), headed with what they are `of`: when
/// GitHub had more checks than were read, it ends saying so rather than
/// that nothing is failing.
async fn failing_text(repo: &github::Repo, of: &str, pr: &Value, complete: bool) -> String {
    let nodes: Vec<Value> = pr
        .pointer(&format!("{CONTEXTS}/nodes"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = format!("# Failing checks of {of}\n\n");
    let mut any = false;
    for node in nodes.iter().filter(|n| check_bucket(n) == "fail") {
        any = true;
        let text = |k: &str| {
            node.get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let name = if text("name").is_empty() {
            text("context")
        } else {
            text("name")
        };
        let link = if text("detailsUrl").is_empty() {
            text("targetUrl")
        } else {
            text("detailsUrl")
        };
        out.push_str(&format!("## {name}\n\n"));
        if !text("description").is_empty() {
            out.push_str(&format!("{}\n\n", text("description")));
        }
        if !link.is_empty() {
            out.push_str(&format!("{link}\n\n"));
        }
        if let Some(job) = JOB.captures(&link).map(|c| c[1].to_string()) {
            let log = github::rest(
                "GET",
                &format!("/repos/{repo}/actions/jobs/{job}/logs"),
                None,
                "M-m failing log",
            )
            .await;
            if let Ok(Value::String(log)) = log {
                let lines: Vec<&str> = log.lines().collect();
                let tail: Vec<String> = lines[lines.len().saturating_sub(25)..]
                    .iter()
                    .map(|l| STAMP.replace(l, "").into_owned())
                    .collect();
                out.push_str(&format!("```\n{}\n```\n\n", tail.join("\n")));
            }
        }
    }
    if !complete {
        out.push_str("_And more checks GitHub didn't send._\n");
    } else if !any {
        out.push_str("_No check is failing now._\n");
    }
    out
}

/// `switchboard pr-view --checks [dir]`: the failing checks, paged.
pub fn checks_main(dir: PathBuf) -> ExitCode {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(match util::block_on(failing_markdown(&dir)) {
            Ok(text) => Content::Markdown {
                text,
                place: super::pr::PLACE.into(),
            },
            Err(why) => Content::Failed(why),
        });
    });
    pager::run(
        Content::Loading("Reading the checks...".into()),
        Some(rx),
        None,
    )
}

// ── The box ─────────────────────────────────────────────────────────────────

/// M-m's box: "Reaching out to GitHub..." until the answer comes, then the
/// answer and what can be done about it, moved through with j and k (or
/// the arrows, Tab, the wheel) and taken with Enter or a click; with
/// nothing to do, any key closes it, as pr-ready.sh's popup did.
pub struct ReadyBox {
    answer: Option<Result<Answer, String>>,
    /// The search `/` opens over its choices (src/ui/menu.rs).
    filter: Filter,
    cursor: usize,
    /// The first choice drawn, past those scrolled off while searching,
    /// and the line it is drawn on.
    first_shown: usize,
    first_line: usize,
    hover: Option<usize>,
    drawn: Option<Drawn>,
}

impl Default for ReadyBox {
    fn default() -> ReadyBox {
        ReadyBox::new()
    }
}

impl ReadyBox {
    pub fn new() -> ReadyBox {
        ReadyBox {
            answer: None,
            filter: Filter::default(),
            cursor: 0,
            first_shown: 0,
            first_line: 0,
            hover: None,
            drawn: None,
        }
    }

    /// The answer has come.
    pub fn answered(&mut self, answer: Result<Answer, String>) {
        self.answer = Some(answer);
        self.cursor = 0;
        self.filter = Filter::default();
    }

    fn choices(&self) -> Vec<Choice> {
        match &self.answer {
            Some(Ok(a)) if !a.choices.is_empty() => {
                let mut c = a.choices.clone();
                c.push(Choice::Close);
                c
            }
            _ => Vec::new(),
        }
    }

    /// The choices the search shows: every one while it is closed.
    fn shown(&self) -> Vec<Choice> {
        let all = self.choices();
        let labels: Vec<&str> = all.iter().map(|c| c.label()).collect();
        self.filter
            .shown(&labels)
            .into_iter()
            .map(|i| all[i])
            .collect()
    }
}

impl Overlay for ReadyBox {
    fn drawn(&self) -> Option<&crate::ui::draw::Drawn> {
        self.drawn.as_ref()
    }

    fn kind(&self) -> &'static str {
        "ready"
    }

    fn stands_out(&self) -> Option<ratatui::layout::Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let mut lines: Vec<BoxLine> = Vec::new();
        match &self.answer {
            None => lines.push(BoxLine::text("Reaching out to GitHub...", look.dim())),
            Some(Err(why)) => {
                for l in draw::wrap(why, 70) {
                    lines.push(BoxLine::text(l, look.text()));
                }
            }
            Some(Ok(answer)) => {
                // While searching, a verdict too long for the screen gives
                // up its last rows, so the search and a match still fit in
                // the box (four rows of its own and two of keys).
                let most = (buf.area.height as usize).saturating_sub(10).max(1);
                let cut = self.filter.is_open() && answer.lines.len() > most;
                let keep = if cut { most - 1 } else { answer.lines.len() };
                for l in &answer.lines[..keep] {
                    lines.push(BoxLine::spans(l.spans.clone()));
                }
                if cut {
                    lines.push(BoxLine::text("…", look.dim()));
                }
            }
        }
        let choices = self.shown();
        let searching = self.filter.is_open();
        if !choices.is_empty() || searching {
            lines.push(BoxLine::blank());
            if searching {
                lines.push(
                    self.filter
                        .line(58, choices.len(), self.choices().len(), look),
                );
                lines.push(BoxLine::blank());
                if choices.is_empty() {
                    lines.push(BoxLine::text(NO_MATCH, look.dim()));
                }
            }
            // While searching, as many matches as the screen has rows for
            // under the verdict and the search, the chosen one among them.
            let (first, fits) = if searching {
                let room = (buf.area.height as usize)
                    .saturating_sub(6 + lines.len())
                    .max(1);
                (self.cursor.saturating_sub(room - 1), room)
            } else {
                (0, choices.len())
            };
            self.first_shown = first;
            self.first_line = lines.len();
            for (i, c) in choices.iter().enumerate().skip(first).take(fits) {
                let chosen = i == self.cursor;
                let mut spans = if chosen {
                    vec![Span::raw(" "), look.pointer(), Span::raw(" ")]
                } else {
                    vec![Span::raw("   ")]
                };
                spans.push(Span::styled(
                    c.label(),
                    if chosen {
                        look.bold_bright()
                    } else {
                        look.text()
                    },
                ));
                let fill = if chosen {
                    Some(look.theme.highlight)
                } else if self.hover == Some(i) {
                    Some(look.theme.panel)
                } else {
                    None
                };
                lines.push(BoxLine::spans(spans).fill(fill).clickable());
            }
        }
        let keys = if searching {
            SEARCH_KEYS
        } else if choices.is_empty() {
            if self.answer.is_none() {
                "esc close"
            } else {
                "any key closes"
            }
        } else {
            "j k move  ↵ choose  esc close"
        };
        let mut spec = BoxSpec::new("Ready to merge?", &lines);
        spec.keys = Some(keys);
        spec.width = 60;
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, _look: &Look) -> Step {
        if !self.choices().is_empty() {
            match self.filter.key(key) {
                Filtered::Changed | Filtered::Closed => {
                    self.cursor = 0;
                    return Step::Redraw;
                }
                Filtered::Not => {}
            }
        }
        let choices = self.shown();
        if self.filter.is_open() {
            return match key.code {
                KeyCode::Down | KeyCode::Tab if !choices.is_empty() => {
                    self.cursor = (self.cursor + 1).min(choices.len() - 1);
                    Step::Redraw
                }
                KeyCode::Char('n') if key.mods == Mods::CTRL && !choices.is_empty() => {
                    self.cursor = (self.cursor + 1).min(choices.len() - 1);
                    Step::Redraw
                }
                KeyCode::Up | KeyCode::BackTab => {
                    self.cursor = self.cursor.saturating_sub(1);
                    Step::Redraw
                }
                KeyCode::Char('p') if key.mods == Mods::CTRL => {
                    self.cursor = self.cursor.saturating_sub(1);
                    Step::Redraw
                }
                KeyCode::Enter => match choices.get(self.cursor) {
                    Some(c) => Step::Close(Outcome::Chose(*c as usize)),
                    None => Step::Same,
                },
                KeyCode::Char('c') if key.mods == Mods::CTRL => Step::Close(Outcome::Dismissed),
                _ => Step::Same,
            };
        }
        if self.answer.is_none() {
            return match key.code {
                KeyCode::Esc | KeyCode::Char('q') => Step::Close(Outcome::Dismissed),
                KeyCode::Char('c') if key.mods == Mods::CTRL => Step::Close(Outcome::Dismissed),
                _ => Step::Same,
            };
        }
        if choices.is_empty() {
            return Step::Close(Outcome::Dismissed);
        }
        let last = choices.len() - 1;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Step::Close(Outcome::Dismissed),
            KeyCode::Char('j') | KeyCode::Down => {
                self.cursor = (self.cursor + 1).min(last);
                Step::Redraw
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.cursor = self.cursor.saturating_sub(1);
                Step::Redraw
            }
            // Tab and Shift-Tab go round, as menu.sh's stops did.
            KeyCode::Tab => {
                self.cursor = if self.cursor >= last {
                    0
                } else {
                    self.cursor + 1
                };
                Step::Redraw
            }
            KeyCode::BackTab => {
                self.cursor = if self.cursor == 0 {
                    last
                } else {
                    self.cursor - 1
                };
                Step::Redraw
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                Step::Close(Outcome::Chose(choices[self.cursor] as usize))
            }
            _ => Step::Same,
        }
    }

    fn paste(&mut self, text: &str, _look: &Look) -> Step {
        if self.filter.paste(text) {
            self.cursor = 0;
            Step::Redraw
        } else {
            Step::Same
        }
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        let hit = drawn.hit(mouse.col, mouse.row);
        let choices = self.shown();
        let first = self.first_line;
        let option = match hit {
            Hit::Line(i, _) if i >= first => {
                Some(i - first + self.first_shown).filter(|o| *o < choices.len())
            }
            _ => None,
        };
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse => {
                if hit == Hit::Outside || hit == Hit::Close {
                    return Step::Close(Outcome::Dismissed);
                }
                match option {
                    Some(i) => Step::Close(Outcome::Chose(choices[i] as usize)),
                    None if self.choices().is_empty() && self.answer.is_some() => {
                        Step::Close(Outcome::Dismissed)
                    }
                    None => Step::Same,
                }
            }
            Gesture::WheelDown if !choices.is_empty() => {
                self.cursor = (self.cursor + 1).min(choices.len() - 1);
                Step::Redraw
            }
            Gesture::WheelUp if !choices.is_empty() => {
                self.cursor = self.cursor.saturating_sub(1);
                Step::Redraw
            }
            Gesture::Move if look.hover => {
                if option == self.hover {
                    Step::Same
                } else {
                    self.hover = option;
                    if let Some(i) = option {
                        self.cursor = i;
                    }
                    Step::Redraw
                }
            }
            _ => Step::Same,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(state: &str, merge_state: &str, mergeable: &str) -> Value {
        json!({
            "number": 12, "title": "Fix the login", "url": "https://github.com/acme/w/pull/12",
            "state": state, "isDraft": false, "mergeable": mergeable, "mergeStateStatus": merge_state,
            "reviewDecision": "REVIEW_REQUIRED", "autoMergeRequest": null, "mergeQueueEntry": null,
            "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "contexts": {
                "checkRunCountsByState": [{ "state": "SUCCESS", "count": 3 }, { "state": "FAILURE", "count": 1 }, { "state": "QUEUED", "count": 0 }],
                "statusContextCountsByState": [{ "state": "PENDING", "count": 1 }],
                "nodes": [
                    { "__typename": "CheckRun", "name": "test", "status": "COMPLETED", "conclusion": "FAILURE", "isRequired": true },
                    { "__typename": "CheckRun", "name": "lint", "status": "COMPLETED", "conclusion": "SUCCESS", "isRequired": true },
                    { "__typename": "StatusContext", "context": "preview", "state": "PENDING", "isRequired": false }
                ]
            } } } }] }
        })
    }

    #[test]
    fn a_blocked_pr_says_why_and_offers_what_can_be_done() {
        let answer = answer_from(&pr("OPEN", "DIRTY", "CONFLICTING"));
        let text = answer.text();
        assert!(text.starts_with("#12 Fix the login\nhttps://github.com/acme/w/pull/12\n\nNot ready: it has merge conflicts."), "{text}");
        assert!(text.contains("Required checks   1 pass, 1 fail"), "{text}");
        assert!(
            text.contains("All checks        3 pass, 1 fail, 1 pending"),
            "{text}"
        );
        assert!(text.contains("Review            review required"), "{text}");
        assert!(
            text.contains("Conflicts         conflicts with the base"),
            "{text}"
        );
        assert_eq!(
            answer.choices,
            vec![Choice::SeeFailing, Choice::ResolveConflicts]
        );
    }

    #[test]
    fn a_ready_pr_needs_no_required_count_and_merged_or_queued_ones_stop_early() {
        let answer = answer_from(&pr("OPEN", "CLEAN", "MERGEABLE"));
        assert!(answer.text().contains("Ready to merge.\n"));
        assert!(answer.text().contains("Required checks   all passing"));
        let merged = answer_from(&pr("MERGED", "CLEAN", "MERGEABLE"));
        assert!(merged.text().ends_with("Already merged."));
        assert!(merged.choices.is_empty());
        let mut queued = pr("OPEN", "CLEAN", "MERGEABLE");
        queued["mergeQueueEntry"] = json!({ "state": "AWAITING_CHECKS", "position": 2 });
        assert!(
            answer_from(&queued)
                .text()
                .ends_with("In the merge queue at position 2, waiting on the queue's checks.")
        );
    }

    #[test]
    fn a_check_failing_past_the_first_page_is_read_and_listed() {
        let passing: Vec<Value> = (0..100)
            .map(|i| json!({ "__typename": "CheckRun", "name": format!("job {i}"), "status": "COMPLETED", "conclusion": "SUCCESS", "isRequired": true }))
            .collect();
        let mut first = pr("OPEN", "BLOCKED", "MERGEABLE");
        first["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"] = json!({
            "checkRunCountsByState": [{ "state": "SUCCESS", "count": 100 }, { "state": "FAILURE", "count": 1 }],
            "statusContextCountsByState": [],
            "pageInfo": { "hasNextPage": true, "endCursor": "c1" },
            "nodes": passing,
        });
        assert!(needs_rest(&first));
        let mut all_passed = first.clone();
        all_passed["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["checkRunCountsByState"] =
            json!([{ "state": "SUCCESS", "count": 101 }]);
        assert!(
            !needs_rest(&all_passed),
            "nothing past the first page to find"
        );

        let mut nodes = passing_nodes(&first);
        nodes.push(json!({ "__typename": "CheckRun", "name": "e2e", "status": "COMPLETED", "conclusion": "FAILURE", "isRequired": true }));
        let pr = with_checks(
            first,
            github::Paged {
                nodes,
                complete: true,
            },
        );
        let answer = answer_from(&pr);
        assert!(
            answer.text().contains("Required checks   100 pass, 1 fail"),
            "{}",
            answer.text()
        );
        assert_eq!(answer.choices, vec![Choice::SeeFailing]);
        let repo = github::Repo {
            owner: "acme".into(),
            name: "w".into(),
        };
        let text = util::block_on(failing_text(&repo, "#12", &pr, true));
        assert!(text.contains("## e2e"), "{text}");
        assert!(!text.contains("No check is failing"), "{text}");
        let text = util::block_on(failing_text(&repo, "#12", &pr, false));
        assert!(
            text.ends_with("## e2e\n\n_And more checks GitHub didn't send._\n"),
            "{text}"
        );
    }

    fn passing_nodes(pr: &Value) -> Vec<Value> {
        pr.pointer(&format!("{CONTEXTS}/nodes"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap()
    }

    fn branch(runs: &[(&str, &str)]) -> Value {
        let nodes: Vec<Value> = runs
            .iter()
            .map(|(name, conclusion)| match *conclusion {
                "RUNNING" => json!({ "__typename": "CheckRun", "name": name, "status": "IN_PROGRESS", "conclusion": null }),
                c => json!({ "__typename": "CheckRun", "name": name, "status": "COMPLETED", "conclusion": c }),
            })
            .collect();
        let by: Vec<Value> = nodes
            .iter()
            .map(|n| {
                let state = n["conclusion"].as_str().unwrap_or("IN_PROGRESS");
                json!({ "state": state, "count": 1 })
            })
            .collect();
        commit_checks(json!({
            "oid": "abcdef1234567890",
            "statusCheckRollup": { "contexts": {
                "checkRunCountsByState": by, "statusContextCountsByState": [], "nodes": nodes,
            } },
        }))
    }

    #[test]
    fn a_branch_with_no_pr_says_how_its_ci_went() {
        let failing = branch(&[("test", "FAILURE"), ("lint", "SUCCESS"), ("e2e", "RUNNING")]);
        let answer = answer_from_branch("main", Some(&failing), Local::Checked);
        let text = answer.text();
        assert!(
            text.starts_with("No PR · CI on main at abcdef1\n\nFailing: 1 of 3."),
            "{text}"
        );
        assert!(
            text.contains("All checks        1 pass, 1 fail, 1 pending"),
            "{text}"
        );
        assert!(!text.contains("GitHub checked"), "{text}");
        assert_eq!(answer.choices, vec![Choice::SeeFailing]);

        let running = branch(&[("lint", "SUCCESS"), ("e2e", "RUNNING")]);
        let answer = answer_from_branch("main", Some(&running), Local::Ahead(2));
        assert!(
            answer.text().contains("Running: 1 of 2."),
            "{}",
            answer.text()
        );
        assert!(
            answer
                .text()
                .ends_with("This tab is 2 commits ahead of what GitHub checked.")
        );
        assert!(answer.choices.is_empty());

        let canceled = branch(&[("lint", "SUCCESS"), ("e2e", "CANCELLED")]);
        let answer = answer_from_branch("main", Some(&canceled), Local::Checked);
        assert!(
            answer.text().contains("Canceled: 1 of 2."),
            "{}",
            answer.text()
        );

        let passing = branch(&[("lint", "SUCCESS")]);
        let answer = answer_from_branch("main", Some(&passing), Local::Elsewhere);
        assert!(answer.text().contains("Passing."), "{}", answer.text());
        assert!(
            answer
                .text()
                .ends_with("This tab's commit isn't the one GitHub checked.")
        );

        let none = commit_checks(json!({ "oid": "abcdef1", "statusCheckRollup": null }));
        let answer = answer_from_branch("main", Some(&none), Local::Checked);
        assert!(answer.text().ends_with("No checks."), "{}", answer.text());

        let missing = answer_from_branch("topic", None, Local::Checked);
        assert_eq!(missing.text(), "No PR, and topic isn't on GitHub.");
        assert!(missing.choices.is_empty());
    }

    #[test]
    fn a_branch_s_failing_checks_are_headed_with_its_commit() {
        let repo = github::Repo {
            owner: "acme".into(),
            name: "w".into(),
        };
        let checks = branch(&[("test", "FAILURE"), ("lint", "SUCCESS")]);
        let text = util::block_on(failing_text(&repo, "`main` at `abcdef1`", &checks, true));
        assert!(
            text.starts_with("# Failing checks of `main` at `abcdef1`\n\n## test\n"),
            "{text}"
        );
        assert!(!text.contains("lint"), "{text}");
    }

    #[test]
    fn the_box_takes_a_choice_and_closes_on_any_key_with_none() {
        let mut b = ReadyBox::new();
        let look = Look::default();
        assert_eq!(
            b.key(Key::new(KeyCode::Char('x'), Mods::NONE), &look),
            Step::Same,
            "still asking"
        );
        b.answered(Ok(answer_from(&pr("OPEN", "DIRTY", "CONFLICTING"))));
        assert_eq!(
            b.key(Key::new(KeyCode::Char('j'), Mods::NONE), &look),
            Step::Redraw
        );
        assert_eq!(
            b.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
            Step::Close(Outcome::Chose(Choice::ResolveConflicts as usize))
        );
        let mut done = ReadyBox::new();
        done.answered(Err("No pull request for this branch.".into()));
        assert_eq!(
            done.key(Key::new(KeyCode::Char('x'), Mods::NONE), &look),
            Step::Close(Outcome::Dismissed)
        );
    }

    #[test]
    fn slash_searches_the_boxs_choices() {
        let look = Look::default();
        let mut b = ReadyBox::new();
        b.answered(Ok(answer_from(&pr("OPEN", "DIRTY", "CONFLICTING"))));
        let all = b.choices().len();
        for c in "/confl".chars() {
            b.key(Key::new(KeyCode::Char(c), Mods::NONE), &look);
        }
        assert_eq!(b.shown(), vec![Choice::ResolveConflicts], "of {all}");
        let mut pasted = ReadyBox::new();
        pasted.answered(Ok(answer_from(&pr("OPEN", "DIRTY", "CONFLICTING"))));
        pasted.key(Key::new(KeyCode::Char('/'), Mods::NONE), &look);
        assert_eq!(pasted.paste("confl", &look), Step::Redraw);
        assert_eq!(
            pasted.shown(),
            vec![Choice::ResolveConflicts],
            "a paste types too"
        );
        let mut short = ReadyBox::new();
        short.answered(Ok(answer_from(&pr("OPEN", "DIRTY", "CONFLICTING"))));
        short.key(Key::new(KeyCode::Char('/'), Mods::NONE), &look);
        let last = short.shown().len() - 1;
        for _ in 0..last {
            short.key(Key::new(KeyCode::Down, Mods::NONE), &look);
        }
        let lines = answer_from(&pr("OPEN", "DIRTY", "CONFLICTING")).lines.len() as u16;
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 80, lines + 10));
        short.draw(&mut buf, &look);
        let s = crate::ui::draw::tests::screen(&buf);
        assert!(
            s.iter().any(|l| l.contains("› Close")),
            "the chosen match in view: {s:#?}"
        );
        let mut tabbed = ReadyBox::new();
        tabbed.answered(Ok(answer_from(&pr("OPEN", "DIRTY", "CONFLICTING"))));
        tabbed.key(Key::new(KeyCode::Char('/'), Mods::NONE), &look);
        tabbed.key(Key::new(KeyCode::Tab, Mods::NONE), &look);
        assert_eq!(tabbed.cursor, 1, "Tab moves among the matches");
        tabbed.key(Key::new(KeyCode::BackTab, Mods::NONE), &look);
        assert_eq!(tabbed.cursor, 0);
        let mut tiny = Buffer::empty(ratatui::layout::Rect::new(0, 0, 80, 14));
        tabbed.draw(&mut tiny, &look);
        let s = crate::ui::draw::tests::screen(&tiny);
        assert!(
            s.iter()
                .any(|l| l.contains('›') && l.contains("See the failing")),
            "a long verdict leaves a match: {s:#?}"
        );
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 80, 30));
        b.draw(&mut buf, &look);
        let s = crate::ui::draw::tests::screen(&buf);
        assert!(s.iter().any(|l| l.contains("› confl▏")), "{s:#?}");
        assert_eq!(
            b.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
            Step::Close(Outcome::Chose(Choice::ResolveConflicts as usize))
        );
    }
}
