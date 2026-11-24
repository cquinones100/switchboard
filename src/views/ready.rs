//! Whether the branch's PR can merge, as the PR view (M-v) says it
//! (tmux/pr-ready.sh, which M-m's box answered until the view took it in,
//! UX 2.0 boards 150 to 153): a verdict, a chip in the view's head with a
//! sentence and its reasons on the line under it, conflicts first, and what
//! can be done about it, which the view's menu offers (see the failing
//! checks, or have the tab's Claude resolve the conflicts).
//!
//! A merged or closed PR has no verdict, its state chip saying it all. A PR
//! already in the merge queue reports its place there and what the queue
//! is doing, and nothing more: its checks, conflicts and review were
//! settled by getting in, and GitHub still calls it mergeable, so the rest
//! would only say "ready" about something already on its way. For anything
//! else the verdict is GitHub's own mergeStateStatus, so it agrees with the
//! merge button, with the reasons explaining it. Required checks are
//! counted apart from the rest, since a failing optional check (a preview
//! deploy) doesn't block the merge and shouldn't read as if it did.
//!
//! The view's own query asks for what this reads (`mergeable`,
//! `mergeStateStatus`, the merge queue entry, each check's `isRequired`),
//! so the PR and whether it can merge come from one query, the checks past
//! its first hundred read a hundred a query (`CHECKS_QUERY`,
//! `github::rest_of`, up to a thousand) only when a check failed or is
//! running, since a failure can be anywhere in GitHub's list. GitHub works
//! out mergeability lazily, so an UNKNOWN is asked again (`MERGE_QUERY`),
//! up to twice more a second apart, as pr-ready.sh did.
//!
//! A branch with no PR (one pushed before opening it, the default branch
//! of a repository that lands straight on it) gets the CI of the commit
//! GitHub has for it instead, from a query on the branch's ref where git
//! pushes it (`BRANCH_QUERY`), with no conflicts or review, which only a PR
//! has. The commit is GitHub's, so the verdict says when this tab's `HEAD`
//! isn't it: ahead of it or behind it by some commits, or elsewhere.
//!
//! `switchboard pr-view --checks` pages the failing checks, each with its
//! description, link and a GitHub Actions job's last lines of log.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};

use super::pager::{self, Content};
use crate::integrations::github;
use crate::ui::kit::{Mark, Tone};
use crate::util;

const SOURCE: &str = "M-v PR checks";

/// A PR's checks a page at a time, for those past the view's first page
/// and for `--checks`, whose list has to have every failing one.
pub const CHECKS_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!, $after: String) { repository(owner: $owner, name: $name) { \
    pullRequest(number: $number) { number \
    commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100, after: $after) { \
    checkRunCountsByState { state count } statusContextCountsByState { state count } \
    pageInfo { hasNextPage endCursor } \
    nodes { __typename \
      ... on CheckRun { name status conclusion detailsUrl title isRequired(pullRequestNumber: $number) } \
      ... on StatusContext { context state targetUrl description isRequired(pullRequestNumber: $number) } \
    } } } } } } } } }";

/// Whether a PR can merge, asked again while GitHub is still working it out.
const MERGE_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) { \
    pullRequest(number: $number) { mergeable mergeStateStatus mergeQueueEntry { state position } } } }";

const BRANCH_QUERY: &str = "query($owner: String!, $name: String!, $qualified: String!, $after: String) { repository(owner: $owner, name: $name) { \
    defaultBranchRef { name } \
    ref(qualifiedName: $qualified) { target { ... on Commit { oid statusCheckRollup { contexts(first: 100, after: $after) { \
    checkRunCountsByState { state count } statusContextCountsByState { state count } \
    pageInfo { hasNextPage endCursor } \
    nodes { __typename \
      ... on CheckRun { name status conclusion detailsUrl title } \
      ... on StatusContext { context state targetUrl description } \
    } } } } } } } }";

/// Where a PR's checks are in `CHECKS_QUERY`'s answer, and in the shape a
/// branch's commit is put in (`commit_checks`).
pub const CONTEXTS: &str = "/commits/nodes/0/commit/statusCheckRollup/contexts";
/// The most pages of checks read, a thousand checks.
pub const CHECK_PAGES: usize = 10;

/// What GitHub's answer about whether something can merge comes to, as
/// the view draws it: a chip in its head (`mark` and `chip`, in `tone`),
/// the sentence under it and its reasons beside that, and what the view's
/// menu then offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub mark: Mark,
    pub chip: String,
    pub tone: Tone,
    pub said: String,
    pub reasons: Vec<String>,
    /// A check is failing: "See the failing checks".
    pub failing: bool,
    /// It has merge conflicts: "Type /resolve-conflicts into this tab's
    /// Claude".
    pub conflicts: bool,
}

impl Verdict {
    /// The chip's words with its mark before them.
    pub fn chip_text(&self) -> String {
        format!("{} {}", self.mark.look().0, self.chip)
    }

    /// The reasons as the line under the head has them.
    pub fn reasons_text(&self) -> String {
        self.reasons.join(" · ")
    }
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
pub fn check_bucket(node: &Value) -> &'static str {
    let text = |k: &str| node.get(k).and_then(Value::as_str).unwrap_or("");
    if node.get("__typename").and_then(Value::as_str) == Some("CheckRun") {
        if text("status") != "COMPLETED" {
            return "pending";
        }
        return bucket(text("conclusion"));
    }
    bucket(text("state"))
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

fn of_bucket(by: &[(&str, u64)], b: &str) -> u64 {
    by.iter().filter(|(k, _)| *k == b).map(|(_, n)| n).sum()
}

/// The required checks among `contexts`' nodes, passed and in all; `None`
/// when GitHub said of none whether it is required (a copy kept before it
/// was asked).
pub fn required(contexts: &Value) -> Option<(u64, u64)> {
    let nodes = contexts.get("nodes").and_then(Value::as_array)?;
    if !nodes.iter().any(|n| n.get("isRequired").is_some()) {
        return None;
    }
    let required: Vec<&Value> = nodes
        .iter()
        .filter(|n| n.get("isRequired").and_then(Value::as_bool) == Some(true))
        .collect();
    let passed = required
        .iter()
        .filter(|n| matches!(check_bucket(n), "pass" | "skipping"))
        .count();
    Some((passed as u64, required.len() as u64))
}

/// The PR's verdict, from the PR as the view's query gives it and its last
/// commit's checks (`contexts`); `None` for a merged or closed PR, and for
/// a copy kept before the view asked whether it can merge.
pub fn verdict(pr: &Value, contexts: &Value) -> Option<Verdict> {
    let text = |k: &str| pr.get(k).and_then(Value::as_str).unwrap_or("");
    if matches!(text("state"), "MERGED" | "CLOSED") {
        return None;
    }
    if let Some(entry) = pr.get("mergeQueueEntry").filter(|e| e.is_object()) {
        return Some(queued(entry));
    }
    let merge_state = pr.get("mergeStateStatus").and_then(Value::as_str)?;
    let base = text("baseRefName");
    let by_bucket = bucket_counts(contexts);
    // GitHub's counts, or where they are missing the checks listed.
    let listed_failing = contexts
        .get("nodes")
        .and_then(Value::as_array)
        .is_some_and(|n| n.iter().any(|c| check_bucket(c) == "fail"));
    let failing = of_bucket(&by_bucket, "fail") > 0 || listed_failing;
    let conflicts = text("mergeable") == "CONFLICTING" || merge_state == "DIRTY";
    let ready = matches!(merge_state, "CLEAN" | "HAS_HOOKS" | "UNSTABLE");
    let (mark, chip, said) = if conflicts {
        (
            Mark::Failed,
            format!("Conflicts with {base}"),
            "Not ready: it has merge conflicts.",
        )
    } else {
        match merge_state {
            "CLEAN" => (Mark::Passed, "Ready to merge".into(), "Ready to merge."),
            "HAS_HOOKS" => (
                Mark::Passed,
                "Ready to merge".into(),
                "Ready to merge (the repo runs merge hooks).",
            ),
            "UNSTABLE" => (
                Mark::Passed,
                "Ready to merge".into(),
                "Ready to merge, though some optional checks are not passing.",
            ),
            "BEHIND" => (
                Mark::Needs,
                format!("Behind {base}"),
                "Not ready: the branch is behind its base and must be updated.",
            ),
            "BLOCKED" => (
                Mark::Failed,
                "Blocked".into(),
                "Not ready: blocked by a required review or check.",
            ),
            "DRAFT" => (
                Mark::Pending,
                "Not ready".into(),
                "Not ready: it is still a draft.",
            ),
            _ => (
                Mark::Pending,
                "Not worked out yet".into(),
                "GitHub has not worked out whether it can merge yet.",
            ),
        }
    };
    let mut reasons = Vec::new();
    if !conflicts && text("mergeable") == "MERGEABLE" {
        reasons.push("no conflicts".to_string());
    }
    match required(contexts) {
        _ if ready => reasons.push("required checks passing".into()),
        Some((passed, total)) if total > 0 && passed == total => {
            reasons.push("required checks passing".into())
        }
        Some((passed, total)) if total > 0 => {
            reasons.push(format!("required {passed} of {total} passed"))
        }
        _ => {}
    }
    match text("reviewDecision") {
        "APPROVED" => reasons.push("approved".into()),
        "CHANGES_REQUESTED" => reasons.push("changes requested".into()),
        "REVIEW_REQUIRED" => reasons.push("review required".into()),
        _ => {}
    }
    if pr.get("autoMergeRequest").is_some_and(|a| !a.is_null()) {
        reasons.push("auto-merge on".into());
    }
    if mark == Mark::Pending && merge_state != "DRAFT" {
        reasons.push("r asks again".into());
    }
    Some(Verdict {
        tone: mark.look().1,
        mark,
        chip,
        said: said.into(),
        reasons,
        failing,
        conflicts,
    })
}

/// A PR in the merge queue: its place, and what the queue is doing.
fn queued(entry: &Value) -> Verdict {
    let state = entry.get("state").and_then(Value::as_str).unwrap_or("");
    let position = entry
        .get("position")
        .map(util::js_string)
        .unwrap_or_default();
    let (mark, said, doing) = match state {
        "UNMERGEABLE" => (
            Mark::Failed,
            format!("In the merge queue at position {position}, but it cannot merge."),
            "it will be removed unless that changes".to_string(),
        ),
        "AWAITING_CHECKS" => (
            Mark::Landing,
            format!("In the merge queue at position {position}."),
            "waiting on the queue's checks".into(),
        ),
        "MERGEABLE" => (
            Mark::Landing,
            format!("In the merge queue at position {position}."),
            "passed and waiting its turn".into(),
        ),
        "LOCKED" => (
            Mark::Landing,
            format!("In the merge queue at position {position}."),
            "merging now".into(),
        ),
        other => (
            Mark::Landing,
            format!("In the merge queue at position {position}."),
            other.to_lowercase().replace('_', " "),
        ),
    };
    Verdict {
        tone: mark.look().1,
        mark,
        chip: "In the merge queue".into(),
        said,
        reasons: vec![doing],
        failing: false,
        conflicts: false,
    }
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

impl Local {
    /// As the view's copy keeps it.
    fn to_json(self) -> Value {
        match self {
            Local::Checked => json!("checked"),
            Local::Ahead(n) => json!({ "ahead": n }),
            Local::Behind(n) => json!({ "behind": n }),
            Local::Elsewhere => json!("elsewhere"),
        }
    }

    fn of_json(v: &Value) -> Local {
        if let Some(n) = v.get("ahead").and_then(Value::as_u64) {
            return Local::Ahead(n);
        }
        if let Some(n) = v.get("behind").and_then(Value::as_u64) {
            return Local::Behind(n);
        }
        match v.as_str() {
            Some("elsewhere") => Local::Elsewhere,
            _ => Local::Checked,
        }
    }

    /// How this tab stands against what GitHub checked, unless it is that.
    fn words(self) -> Option<String> {
        let commits = |n: u64| format!("{n} commit{}", if n == 1 { "" } else { "s" });
        match self {
            Local::Checked => None,
            Local::Ahead(n) => Some(format!(
                "this tab is {} ahead of what GitHub checked",
                commits(n)
            )),
            Local::Behind(n) => Some(format!(
                "this tab is {} behind what GitHub checked",
                commits(n)
            )),
            Local::Elsewhere => Some("this tab's commit isn't the one GitHub checked".into()),
        }
    }
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

/// Whether a copy the view keeps is a branch with no PR (`branch_ci`'s).
pub fn is_branch(copy: &Value) -> bool {
    copy.get("noPr").and_then(Value::as_bool) == Some(true)
}

/// The branch's verdict, from what `branch_ci` read: how its CI went on
/// GitHub's commit for it, and how this tab stands against that commit.
pub fn branch_verdict(copy: &Value) -> Verdict {
    let branch = copy.get("branch").and_then(Value::as_str).unwrap_or("");
    let Some(checks) = copy.get("checks").filter(|c| c.is_object()) else {
        return Verdict {
            mark: Mark::Idle,
            chip: "Not on GitHub".into(),
            tone: Tone::Dim,
            said: format!("{branch} isn't on GitHub."),
            reasons: vec!["push it and open a PR, then M-v again".into()],
            failing: false,
            conflicts: false,
        };
    };
    let oid = checks
        .pointer("/commits/nodes/0/commit/oid")
        .and_then(Value::as_str)
        .unwrap_or("");
    let contexts = checks.pointer(CONTEXTS).cloned().unwrap_or(Value::Null);
    let by_bucket = bucket_counts(&contexts);
    let total: u64 = by_bucket.iter().map(|(_, n)| n).sum();
    let of = |b: &str| of_bucket(&by_bucket, b);
    let (failing, running, canceled) = (of("fail"), of("pending"), of("cancel"));
    let (mark, chip, said) = if total == 0 {
        (Mark::Idle, "No checks", "No checks.".to_string())
    } else if failing > 0 {
        (
            Mark::Failed,
            "CI failing",
            format!("Failing: {failing} of {total}."),
        )
    } else if running > 0 {
        (
            Mark::Pending,
            "CI running",
            format!("Running: {running} of {total}."),
        )
    } else if canceled > 0 {
        (
            Mark::Needs,
            "CI canceled",
            format!("Canceled: {canceled} of {total}."),
        )
    } else {
        (Mark::Passed, "CI passing", "Passing.".to_string())
    };
    let mut reasons = vec![format!("CI on {branch} at {}", short(oid))];
    reasons.extend(Local::of_json(copy.get("local").unwrap_or(&Value::Null)).words());
    if copy.get("complete").and_then(Value::as_bool) == Some(false) {
        reasons.push("more checks than GitHub sent".into());
    }
    Verdict {
        tone: mark.look().1,
        mark,
        chip: chip.into(),
        said,
        reasons,
        failing: failing > 0,
        conflicts: false,
    }
}

/// Whether the checks GitHub left off the first page of `contexts` are
/// needed: its counts say a check failed or is still running, so the list
/// of them has to have every check, and there are more pages. A PR whose
/// checks all passed, or that has a hundred or fewer, costs one query.
pub fn needs_rest(contexts: &Value) -> bool {
    let unsettled = bucket_counts(contexts)
        .iter()
        .any(|(b, n)| *n > 0 && matches!(*b, "fail" | "pending"));
    let more = contexts
        .pointer("/pageInfo/hasNextPage")
        .and_then(Value::as_bool)
        == Some(true);
    unsettled && more
}

/// `contexts` (a first page) with every check after it read too, from
/// `query` asked again, its checks at `at` in the query's data; and whether
/// every check is there.
async fn every_check(
    contexts: Value,
    query: &str,
    variables: &Value,
    at: &str,
) -> Result<(Value, bool), String> {
    if !needs_rest(&contexts) {
        return Ok((contexts, true));
    }
    let paged = github::rest_of(query, variables, at, &contexts, CHECK_PAGES, SOURCE)
        .await
        .map_err(|e| e.to_string())?;
    let mut contexts = contexts;
    contexts["nodes"] = Value::Array(paged.nodes);
    Ok((contexts, paged.complete))
}

/// The PR's checks (`contexts`, the view's first page) with every one read
/// when any failed or is running; and whether every check is there.
pub async fn pr_checks(
    repo: &github::Repo,
    number: u64,
    contexts: Value,
) -> Result<(Value, bool), String> {
    let variables = json!({ "owner": repo.owner, "name": repo.name, "number": number });
    every_check(
        contexts,
        CHECKS_QUERY,
        &variables,
        &format!("/repository/pullRequest{CONTEXTS}"),
    )
    .await
}

/// Whether the PR can merge, asked again up to twice a second apart while
/// GitHub answers UNKNOWN, as `pr` carries it: its fields replaced by the
/// last answer.
pub async fn settle(repo: &github::Repo, number: u64, pr: &mut Value) {
    let variables = json!({ "owner": repo.owner, "name": repo.name, "number": number });
    // A merged or closed PR, or one the queue holds, shows no verdict of
    // GitHub's mergeability, so it isn't waited on.
    let settled = matches!(
        pr.get("state").and_then(Value::as_str),
        Some("MERGED" | "CLOSED")
    ) || pr.get("mergeQueueEntry").is_some_and(Value::is_object);
    if settled {
        return;
    }
    for _ in 0..2 {
        if pr.get("mergeable").and_then(Value::as_str) != Some("UNKNOWN") {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        let Ok(data) = github::graphql(MERGE_QUERY, variables.clone(), SOURCE).await else {
            return;
        };
        let Some(answer) = data.pointer("/repository/pullRequest") else {
            return;
        };
        for key in ["mergeable", "mergeStateStatus", "mergeQueueEntry"] {
            pr[key] = answer.get(key).cloned().unwrap_or(Value::Null);
        }
    }
}

/// What the PR view keeps for a branch with no PR: the checks of the
/// commit GitHub has for it (`commit_checks`'s shape, null when GitHub has
/// no such branch), whether every check is there, how this tab stands
/// against that commit, and how far the branch is ahead of the default one.
pub async fn branch_ci(dir: &Path) -> Result<Value, String> {
    let pushed = github::pushed_branch(dir)
        .await
        .map_err(|e| e.to_string())?;
    let Some((repo, branch)) = pushed else {
        return Err("No PR, and this checkout isn't on a branch.".into());
    };
    let variables = json!({
        "owner": repo.owner, "name": repo.name,
        "qualified": format!("refs/heads/{branch}"),
    });
    let data = github::graphql(BRANCH_QUERY, variables.clone(), SOURCE)
        .await
        .map_err(|e| e.to_string())?;
    let default = data
        .pointer("/repository/defaultBranchRef/name")
        .and_then(Value::as_str)
        .map(str::to_string);
    let ahead = match &default {
        Some(d) if *d != branch => ahead_of(dir, d).await,
        _ => None,
    };
    let mut copy = json!({
        "noPr": true, "owner": repo.owner, "name": repo.name, "branch": branch, "checks": null, "complete": true,
        "local": Local::Checked.to_json(), "defaultBranch": default, "aheadOfDefault": ahead,
    });
    let Some(commit) = data
        .pointer("/repository/ref/target")
        .filter(|t| t.get("oid").is_some())
        .cloned()
    else {
        return Ok(copy);
    };
    let oid = commit
        .get("oid")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut checks = commit_checks(commit);
    if let Some(first) = checks.pointer(CONTEXTS).cloned() {
        let (contexts, complete) = every_check(
            first,
            BRANCH_QUERY,
            &variables,
            "/repository/ref/target/statusCheckRollup/contexts",
        )
        .await?;
        if let Some(c) = checks.pointer_mut(CONTEXTS) {
            *c = contexts;
        }
        copy["complete"] = json!(complete);
    }
    copy["checks"] = checks;
    copy["local"] = local_against(dir, &oid).await.to_json();
    Ok(copy)
}

/// How many commits `HEAD` has that the default branch, as origin last
/// had it, doesn't; none when git can't tell.
async fn ahead_of(dir: &Path, default: &str) -> Option<u64> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(["--no-optional-locks", "rev-list", "--count"])
        .arg(format!("refs/remotes/origin/{default}..HEAD"));
    let output = util::run_async(&mut cmd, Some(Duration::from_secs(10)))
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
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

static JOB: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/actions/runs/\d+/job/(\d+)").unwrap());
static STAMP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{4}-\d\d-\d\dT[0-9:.]+Z ").unwrap());

/// The failing checks as Markdown: each with its description and link, and
/// a GitHub Actions job's last 25 lines of log, as pr-ready.sh's
/// `failing` showed them in less. The branch's PR's, else the branch's
/// own commit's.
pub async fn failing_markdown(dir: &Path) -> Result<String, String> {
    let found = github::branch_pr(dir, SOURCE)
        .await
        .map_err(|e| e.to_string())?;
    let Some((repo, pr)) = found else {
        let copy = branch_ci(dir).await?;
        let branch = copy.get("branch").and_then(Value::as_str).unwrap_or("");
        let Some(checks) = copy.get("checks").filter(|c| c.is_object()) else {
            return Err(format!("No PR, and {branch} isn't on GitHub."));
        };
        let oid = checks
            .pointer("/commits/nodes/0/commit/oid")
            .and_then(Value::as_str)
            .unwrap_or("");
        let text = |k: &str| {
            copy.get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let repo = github::Repo {
            owner: text("owner"),
            name: text("name"),
        };
        let complete = copy.get("complete").and_then(Value::as_bool) != Some(false);
        let of = format!("`{branch}` at `{}`", short(oid));
        let contexts = checks.pointer(CONTEXTS).cloned().unwrap_or(Value::Null);
        return Ok(failing_text(&repo, &of, &contexts, complete).await);
    };
    let variables = json!({ "owner": repo.owner, "name": repo.name, "number": pr.number });
    let data = github::graphql(CHECKS_QUERY, variables.clone(), SOURCE)
        .await
        .map_err(|e| e.to_string())?;
    let first = data
        .pointer(&format!("/repository/pullRequest{CONTEXTS}"))
        .cloned()
        .unwrap_or(Value::Null);
    let (contexts, complete) = every_check(
        first,
        CHECKS_QUERY,
        &variables,
        &format!("/repository/pullRequest{CONTEXTS}"),
    )
    .await?;
    Ok(failing_text(&repo, &format!("#{}", pr.number), &contexts, complete).await)
}

/// `failing_markdown`'s text for `contexts` (a PR's or a branch's
/// commit's), headed with what they are `of`: when GitHub had more checks
/// than were read, it ends saying so rather than that nothing is failing.
async fn failing_text(repo: &github::Repo, of: &str, contexts: &Value, complete: bool) -> String {
    let nodes: Vec<Value> = contexts
        .get("nodes")
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
        let about = if text("description").is_empty() {
            text("title")
        } else {
            text("description")
        };
        if !about.is_empty() {
            out.push_str(&format!("{about}\n\n"));
        }
        if !link.is_empty() {
            out.push_str(&format!("{link}\n\n"));
        }
        if let Some(job) = JOB.captures(&link).map(|c| c[1].to_string()) {
            let log = github::rest(
                "GET",
                &format!("/repos/{repo}/actions/jobs/{job}/logs"),
                None,
                "M-v failing log",
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

#[cfg(test)]
mod tests {
    use super::*;

    fn contexts() -> Value {
        json!({
            "checkRunCountsByState": [{ "state": "SUCCESS", "count": 3 }, { "state": "FAILURE", "count": 1 }, { "state": "QUEUED", "count": 0 }],
            "statusContextCountsByState": [{ "state": "PENDING", "count": 1 }],
            "nodes": [
                { "__typename": "CheckRun", "name": "test", "status": "COMPLETED", "conclusion": "FAILURE", "isRequired": true },
                { "__typename": "CheckRun", "name": "lint", "status": "COMPLETED", "conclusion": "SUCCESS", "isRequired": true },
                { "__typename": "StatusContext", "context": "preview", "state": "PENDING", "isRequired": false }
            ]
        })
    }

    fn pr(state: &str, merge_state: &str, mergeable: &str) -> Value {
        json!({
            "number": 12, "title": "Fix the login", "state": state, "isDraft": false,
            "baseRefName": "main", "mergeable": mergeable, "mergeStateStatus": merge_state,
            "reviewDecision": "CHANGES_REQUESTED", "autoMergeRequest": null, "mergeQueueEntry": null,
        })
    }

    #[test]
    fn a_pr_with_conflicts_says_so_first_with_its_reasons() {
        let v = verdict(&pr("OPEN", "DIRTY", "CONFLICTING"), &contexts()).unwrap();
        assert_eq!(v.chip_text(), "✕ Conflicts with main");
        assert_eq!(v.said, "Not ready: it has merge conflicts.");
        assert_eq!(
            v.reasons_text(),
            "required 1 of 2 passed · changes requested"
        );
        assert!(v.failing && v.conflicts);
    }

    #[test]
    fn a_ready_pr_needs_no_required_count() {
        let mut clean = pr("OPEN", "CLEAN", "MERGEABLE");
        clean["reviewDecision"] = json!("APPROVED");
        let v = verdict(&clean, &json!({})).unwrap();
        assert_eq!(v.chip_text(), "✓ Ready to merge");
        assert_eq!(v.said, "Ready to merge.");
        assert_eq!(
            v.reasons_text(),
            "no conflicts · required checks passing · approved"
        );
        assert!(!v.failing && !v.conflicts);
    }

    #[test]
    fn merged_closed_and_unasked_prs_have_no_verdict_and_queued_ones_stop_early() {
        assert!(verdict(&pr("MERGED", "CLEAN", "MERGEABLE"), &contexts()).is_none());
        assert!(verdict(&pr("CLOSED", "CLEAN", "MERGEABLE"), &contexts()).is_none());
        let kept = json!({ "number": 3, "state": "OPEN" });
        assert!(verdict(&kept, &contexts()).is_none(), "a copy from before");
        let mut queued = pr("OPEN", "CLEAN", "MERGEABLE");
        queued["mergeQueueEntry"] = json!({ "state": "AWAITING_CHECKS", "position": 2 });
        let v = verdict(&queued, &contexts()).unwrap();
        assert_eq!(v.chip_text(), "▲ In the merge queue");
        assert_eq!(v.said, "In the merge queue at position 2.");
        assert_eq!(v.reasons_text(), "waiting on the queue's checks");
        assert!(!v.failing, "the queue settled the checks");
    }

    #[test]
    fn unknown_says_github_is_still_working_it_out() {
        let v = verdict(&pr("OPEN", "UNKNOWN", "UNKNOWN"), &json!({})).unwrap();
        assert_eq!(
            v.said,
            "GitHub has not worked out whether it can merge yet."
        );
        assert!(v.reasons.contains(&"r asks again".to_string()));
    }

    #[test]
    fn a_failing_check_past_the_first_page_is_needed() {
        let mut first = json!({
            "checkRunCountsByState": [{ "state": "SUCCESS", "count": 100 }, { "state": "FAILURE", "count": 1 }],
            "statusContextCountsByState": [],
            "pageInfo": { "hasNextPage": true, "endCursor": "c1" },
            "nodes": [],
        });
        assert!(needs_rest(&first));
        first["checkRunCountsByState"] = json!([{ "state": "SUCCESS", "count": 101 }]);
        assert!(!needs_rest(&first), "nothing past the first page to find");
    }

    fn branch(runs: &[(&str, &str)], local: Local) -> Value {
        let nodes: Vec<Value> = runs
            .iter()
            .map(|(name, conclusion)| match *conclusion {
                "RUNNING" => json!({ "__typename": "CheckRun", "name": name, "status": "IN_PROGRESS", "conclusion": null }),
                c => json!({ "__typename": "CheckRun", "name": name, "status": "COMPLETED", "conclusion": c }),
            })
            .collect();
        let by: Vec<Value> = nodes
            .iter()
            .map(|n| json!({ "state": n["conclusion"].as_str().unwrap_or("IN_PROGRESS"), "count": 1 }))
            .collect();
        json!({
            "noPr": true, "branch": "main", "complete": true, "local": local.to_json(),
            "checks": commit_checks(json!({
                "oid": "abcdef1234567890",
                "statusCheckRollup": { "contexts": {
                    "checkRunCountsByState": by, "statusContextCountsByState": [], "nodes": nodes,
                } },
            })),
        })
    }

    #[test]
    fn a_branch_with_no_pr_says_how_its_ci_went() {
        let failing = branch(
            &[("test", "FAILURE"), ("lint", "SUCCESS"), ("e2e", "RUNNING")],
            Local::Checked,
        );
        let v = branch_verdict(&failing);
        assert_eq!(v.chip_text(), "✕ CI failing");
        assert_eq!(v.said, "Failing: 1 of 3.");
        assert_eq!(v.reasons_text(), "CI on main at abcdef1");
        assert!(v.failing);

        let running = branch(&[("lint", "SUCCESS"), ("e2e", "RUNNING")], Local::Ahead(2));
        let v = branch_verdict(&running);
        assert_eq!(v.said, "Running: 1 of 2.");
        assert_eq!(
            v.reasons_text(),
            "CI on main at abcdef1 · this tab is 2 commits ahead of what GitHub checked"
        );
        assert!(!v.failing);

        let canceled = branch(
            &[("lint", "SUCCESS"), ("e2e", "CANCELLED")],
            Local::Behind(1),
        );
        let v = branch_verdict(&canceled);
        assert_eq!(v.said, "Canceled: 1 of 2.");
        assert!(
            v.reasons_text()
                .ends_with("this tab is 1 commit behind what GitHub checked")
        );

        let passing = branch(&[("lint", "SUCCESS")], Local::Elsewhere);
        let v = branch_verdict(&passing);
        assert_eq!(v.said, "Passing.");
        assert!(
            v.reasons_text()
                .ends_with("this tab's commit isn't the one GitHub checked")
        );

        let missing = json!({ "noPr": true, "branch": "topic", "checks": null });
        assert_eq!(branch_verdict(&missing).said, "topic isn't on GitHub.");
    }

    #[test]
    fn the_failing_checks_are_headed_with_what_they_are_of() {
        let repo = github::Repo {
            owner: "acme".into(),
            name: "w".into(),
        };
        let copy = branch(&[("test", "FAILURE"), ("lint", "SUCCESS")], Local::Checked);
        let contexts = copy["checks"].pointer(CONTEXTS).cloned().unwrap();
        let text = util::block_on(failing_text(&repo, "`main` at `abcdef1`", &contexts, true));
        assert!(
            text.starts_with("# Failing checks of `main` at `abcdef1`\n\n## test\n"),
            "{text}"
        );
        assert!(!text.contains("lint"), "{text}");
        let text = util::block_on(failing_text(&repo, "#12", &contexts, false));
        assert!(
            text.ends_with("_And more checks GitHub didn't send._\n"),
            "{text}"
        );
    }

    /// Every GraphQL query in `src/views/` (any string literal Rust parses
    /// there whose text starts with `query` or `mutation`) closes each brace
    /// it opens outside its own string arguments, so a query left open fails
    /// here rather than in the view, as `CHECKS_QUERY` once did in the failing
    /// checks and in M-v on a PR with checks past the first page. The files
    /// are parsed with syn, so each literal is read as Rust decodes it. syn
    /// leaves a macro's arguments as tokens, whose strings it can't put
    /// together as the macro would, so a string in a macro (`format!`,
    /// `concat!`) that decodes to a query fails the test too: each query stays
    /// a plain literal this test can check.
    #[test]
    fn every_views_query_closes_its_braces() {
        struct Queries {
            literals: Vec<String>,
            in_macros: Vec<String>,
        }
        impl<'ast> syn::visit::Visit<'ast> for Queries {
            fn visit_lit_str(&mut self, literal: &'ast syn::LitStr) {
                let text = literal.value();
                let start = text.trim_start();
                if start.starts_with("query") || start.starts_with("mutation") {
                    self.literals.push(text);
                }
            }
            fn visit_macro(&mut self, mac: &'ast syn::Macro) {
                fn strings_in(tokens: proc_macro2::TokenStream, found: &mut Vec<String>) {
                    for token in tokens {
                        match token {
                            proc_macro2::TokenTree::Group(group) => {
                                strings_in(group.stream(), found)
                            }
                            proc_macro2::TokenTree::Literal(literal) => {
                                if let Ok(string) =
                                    syn::parse_str::<syn::LitStr>(&literal.to_string())
                                {
                                    found.push(string.value());
                                }
                            }
                            _ => {}
                        }
                    }
                }
                let mut strings = Vec::new();
                strings_in(mac.tokens.clone(), &mut strings);
                for text in strings {
                    let start = text.trim_start();
                    let rest = ["query", "mutation"]
                        .iter()
                        .find_map(|keyword| start.strip_prefix(keyword));
                    let is_query = rest.is_some_and(|rest| {
                        rest.chars()
                            .next()
                            .is_some_and(|c| !c.is_alphanumeric() && c != '_')
                    });
                    if is_query {
                        self.in_macros.push(text);
                    }
                }
            }
        }
        fn rust_files(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    rust_files(&path, found);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    found.push(path);
                }
            }
        }
        // The views' own queries, and the ticket sources' their pickers ask.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&src.join("views"), &mut files);
        rust_files(&src.join("tickets"), &mut files);
        let graphql_string = Regex::new(r#"(?s)"(?:[^"\\]|\\.)*""#).unwrap();
        let mut found = Vec::new();
        for file in files {
            let source = std::fs::read_to_string(&file).unwrap();
            let parsed = syn::parse_file(&source).unwrap();
            let mut queries = Queries {
                literals: Vec::new(),
                in_macros: Vec::new(),
            };
            syn::visit::Visit::visit_file(&mut queries, &parsed);
            assert!(
                queries.in_macros.is_empty(),
                "a query in {} is inside a macro, where this test can't read it: {:?}",
                file.display(),
                queries.in_macros
            );
            for text in queries.literals {
                let outside_strings = graphql_string.replace_all(&text, "\"\"");
                let mut depth = 0usize;
                for brace in outside_strings.chars().filter(|c| matches!(c, '{' | '}')) {
                    if brace == '{' {
                        depth += 1;
                    } else {
                        depth = depth.checked_sub(1).unwrap_or_else(|| {
                            panic!(
                                "a query in {} closes a brace it never opened: {text}",
                                file.display()
                            )
                        });
                    }
                }
                assert_eq!(
                    depth,
                    0,
                    "a query in {} leaves {depth} braces open: {text}",
                    file.display()
                );
                found.push(text);
            }
        }
        for expected in [CHECKS_QUERY, crate::tickets::linear::YOUR_TICKETS] {
            assert!(found.iter().any(|text| text == expected), "{expected}");
        }
    }
}
