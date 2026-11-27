//! The merge queue's trains, one step at a time: each step looks once at
//! what the queue holds and moves it on, keeping nothing but the queue's
//! file, so a server restarting mid-train takes it up where it was.
//!
//! With no train under test, a step takes the next group (the first split
//! of a red train, else the first `TRAIN_MAX` PRs queued that are ready), asks GitHub for
//! each PR's head, base and own checks, and builds a train of those ready:
//! open, on the default branch, their own checks passed. A PR whose checks
//! still run waits for a later train, and so does one whose head isn't the
//! one it was queued with, until `land-lock queue-pr` queues it again at
//! its new head: a train's push runs the workflows of the commits it holds
//! with the repository's secrets, so it holds only heads someone queued.
//! One whose checks failed, whose head is a fork's branch, that targets
//! another branch, or that doesn't merge cleanly onto the train is dropped
//! and its tab told. The train is built in the main clone's git directory
//! alone, with no worktree, so nothing checked out there changes: the
//! default branch and the PRs' heads fetched from origin into
//! `refs/switchboard/merge-queue/`, each PR merged onto the last
//! (`git merge-tree --write-tree`, `git commit-tree`), and the tip pushed
//! to `ci/train-<its first 12 hex digits>`, whose push runs the
//! repository's CI.
//!
//! With a train under test, a step looks at its checks once (`wait_ci`'s
//! `look`). Green on two looks in a row (as `wait-ci` waits, since a job
//! that needs another appears only once that one ends), and the default branch still where the train was built,
//! it squash-merges each PR in order with the head the train tested
//! (GitHub refusing a PR whose head has moved since), so the default branch
//! ends on the tree the train tested; a PR whose head moved stops the
//! merging there, it waiting to be queued again and those after it staying
//! queued for the next train, since what follows was tested with it. The default branch having
//! moved, the train is tested again. Red the first time, its failed jobs
//! are run again once the run has ended (GitHub's "Re-run failed jobs"), since a test that fails
//! now and then on a busy runner shouldn't drop a PR that is fine, and the
//! train waits on that attempt as on the first; a failure showing before
//! that run has ended again is the first attempt's, until the rerun's own
//! check runs replace it. Red again, or with no rerun to be had, a train of
//! one drops its PR, and a longer one is split into halves tested first,
//! until the PR at fault is the only one dropped. A PR taken out of the queue ends its train. A
//! train with no CI after `GRACE`, or none finished after `TIMEOUT`, holds
//! the queue until something in it changes, and so do a push of a train or
//! a merge failing `TRIES` times in a row, tried again on each step until
//! then. Every way a train ends deletes its branch.
//!
//! While a train is under test, a step also builds the train ahead, as
//! GitHub's merge queue does: the next group (a split's other half first)
//! merged onto the train under test's commit and pushed to a branch of its
//! own, so its CI runs beside that one's, a CI run taking long enough that
//! testing one train at a time held a dozen PRs for half an hour. Nothing
//! looks at its checks until it is the train under test, so its failure on
//! top of a train that then fails drops nothing. Once every PR of the
//! train under test merged, with the default branch on the tree that train
//! tested, the train ahead becomes the train under test, its CI counting as
//! it stands. Any other end of the train under test (red, the default
//! branch moved, a PR taken out, the queue held), or a PR of the train
//! ahead taken out, ends the train ahead: its run cancelled, its branch
//! deleted, its PRs back in line (a split's behind the splits there are).
//! There is one train ahead at most.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, anyhow};

use super::{Queue, Rerun, TRAIN_MAX, Tested, Train};
use crate::cmd::wait_ci::Seen;
use crate::util;
use crate::worktree::git;

/// How long a train may have no CI at all, and how long its CI may run.
pub const GRACE: i64 = 15 * 60 * 1000;
pub const TIMEOUT: i64 = 120 * 60 * 1000;
/// How long a PR queued with no checks on its head waits for one before it
/// counts as ready.
pub const NO_CHECKS: i64 = 3 * 60 * 1000;

/// Where the train's refs are fetched to in the main clone.
const REFS: &str = "refs/switchboard/merge-queue";

/// What the request audit calls the queue's requests.
pub const SOURCE: &str = "merge queue";

/// A PR as GitHub has it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrNow {
    pub state: PrState,
    pub head: String,
    pub base: String,
    /// Its head is a branch of the repository itself, not of a fork.
    pub from_here: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrState {
    Open,
    Merged,
    Closed,
}

/// What the queue asks of GitHub, apart so the tests can answer it. Its
/// futures run on one thread (`Handle::block_on`), so need no `Send`.
#[allow(async_fn_in_trait)]
pub trait Forge {
    async fn pr(&self, number: u64) -> Result<PrNow>;
    /// The base branch's required checks, `None` when it requires none
    /// (no protection), when every check counts; an error when GitHub
    /// couldn't be asked, which ends the step.
    async fn required(&self, branch: &str) -> Result<Option<Vec<String>>>;
    /// A commit's checks at one look, `required` alone when given.
    async fn checks(&self, sha: &str, required: Option<&[String]>) -> Result<Seen>;
    /// Squash-merges a PR, refused unless its head is still `head`.
    async fn merge(&self, number: u64, head: &str) -> Result<(), MergeError>;
    /// Whether a GitHub Actions run has ended, every job of it.
    async fn run_finished(&self, run: &str) -> Result<bool>;
    /// Runs a GitHub Actions run's failed jobs again.
    async fn rerun(&self, run: &str) -> Result<()>;
    /// Cancels the GitHub Actions runs on a commit that haven't ended,
    /// which deleting their branch doesn't.
    async fn cancel(&self, commit: &str) -> Result<()>;
    /// A link to a GitHub Actions run, for what is said of a red train.
    fn run_url(&self, run: &str) -> String;
}

/// GitHub's own answers, through src/integrations.
pub struct GitHub {
    /// `owner/name`.
    pub repo: String,
}

impl Forge for GitHub {
    async fn pr(&self, number: u64) -> Result<PrNow> {
        use crate::integrations::github;
        let path = format!("repos/{}/pulls/{number}", self.repo);
        let pr = github::rest("GET", &path, None, SOURCE).await?;
        let text = |at: &str| {
            pr.pointer(at)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let merged = pr.get("merged").and_then(|m| m.as_bool()) == Some(true);
        Ok(PrNow {
            state: match (merged, text("/state").as_str()) {
                (true, _) => PrState::Merged,
                (false, "open") => PrState::Open,
                _ => PrState::Closed,
            },
            head: text("/head/sha"),
            base: text("/base/ref"),
            // A fork deleted since leaves no head repository, read as
            // another's.
            from_here: !text("/head/repo/full_name").is_empty()
                && text("/head/repo/full_name").eq_ignore_ascii_case(&text("/base/repo/full_name")),
        })
    }

    async fn required(&self, branch: &str) -> Result<Option<Vec<String>>> {
        use crate::integrations::github;
        let path = format!("repos/{}/branches/{branch}", self.repo);
        let answer = github::rest("GET", &path, None, SOURCE).await?;
        let names: Vec<String> = answer
            .pointer("/protection/required_status_checks/contexts")
            .and_then(|c| c.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|c| c.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        Ok((!names.is_empty()).then_some(names))
    }

    async fn checks(&self, sha: &str, required: Option<&[String]>) -> Result<Seen> {
        use crate::cmd::wait_ci;
        Ok(match required {
            Some(names) => wait_ci::look_required(&self.repo, sha, names, SOURCE).await?,
            None => wait_ci::look(&self.repo, sha, SOURCE).await?.1,
        })
    }

    async fn merge(&self, number: u64, head: &str) -> Result<(), MergeError> {
        use crate::integrations::github;
        let path = format!("repos/{}/pulls/{number}/merge", self.repo);
        let body = serde_json::json!({ "merge_method": "squash", "sha": head });
        github::rest("PUT", &path, Some(&body), SOURCE)
            .await
            .map_err(merge_error)?;
        Ok(())
    }

    async fn run_finished(&self, run: &str) -> Result<bool> {
        use crate::integrations::github;
        let path = format!("repos/{}/actions/runs/{run}", self.repo);
        let answer = github::rest("GET", &path, None, SOURCE).await?;
        Ok(answer.get("status").and_then(|s| s.as_str()) == Some("completed"))
    }

    async fn rerun(&self, run: &str) -> Result<()> {
        use crate::integrations::github;
        let path = format!("repos/{}/actions/runs/{run}/rerun-failed-jobs", self.repo);
        github::rest("POST", &path, None, SOURCE).await?;
        Ok(())
    }

    async fn cancel(&self, commit: &str) -> Result<()> {
        use crate::integrations::github;
        let path = format!(
            "repos/{}/actions/runs?head_sha={commit}&per_page=100",
            self.repo
        );
        let answer = github::rest("GET", &path, None, SOURCE).await?;
        let runs = answer
            .get("workflow_runs")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        for run in runs {
            let ended = run.get("status").and_then(|s| s.as_str()) == Some("completed");
            let Some(id) = run.get("id").and_then(|i| i.as_u64()).filter(|_| !ended) else {
                continue;
            };
            let path = format!("repos/{}/actions/runs/{id}/cancel", self.repo);
            // One that ended meanwhile can't be cancelled, which is fine.
            let _ = github::rest("POST", &path, None, SOURCE).await;
        }
        Ok(())
    }

    fn run_url(&self, run: &str) -> String {
        format!("https://github.com/{}/actions/runs/{run}", self.repo)
    }
}

/// Why a merge didn't go through.
#[derive(Debug)]
pub enum MergeError {
    /// The base branch requires PRs to be up to date, which a PR waiting in
    /// the queue isn't: the queue holds until that rule is turned off.
    Strict(String),
    /// GitHub won't merge the PR as it is (a review it requires, a draft),
    /// in its own words: the PR is dropped rather than the queue held.
    Refused(String),
    /// Anything that may pass (the network, a base just merged into).
    Failed(anyhow::Error),
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeError::Refused(why) | MergeError::Strict(why) => f.write_str(why),
            MergeError::Failed(e) => write!(f, "{e}"),
        }
    }
}

/// A merge's error as `MergeError`: GitHub's 405 or 422 is a refusal,
/// unless it says the base branch was just modified (by the train's own
/// merge before it) or the PR isn't mergeable yet, which pass on a second
/// try.
pub fn merge_error(e: anyhow::Error) -> MergeError {
    let words = e.to_string();
    // GitHub words it "Head branch is out of date" (or "not up to date").
    if words.contains("out of date") || words.contains("not up to date") {
        return MergeError::Strict(words);
    }
    let refusal = (words.contains("answered 405") || words.contains("answered 422"))
        && !words.contains("Base branch was modified")
        // GitHub works out mergeability lazily, so right after a merge into
        // the base it may answer this for a PR it merges a moment later.
        && !words.contains("not mergeable");
    if refusal {
        MergeError::Refused(words)
    } else {
        MergeError::Failed(e)
    }
}

/// A repository's queue: its shared git directory and its default branch.
pub struct Repo {
    pub common: PathBuf,
    pub branch: String,
}

/// What a step tells a PR's tab: the worktree and pane that queued it, and
/// what happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Told {
    pub number: u64,
    pub path: String,
    pub pane: Option<String>,
    pub words: String,
}

/// One step, as the module's comment says, at `now` (ms): what it tells
/// the PRs' tabs.
pub async fn step(at: &Repo, forge: &impl Forge, now: i64) -> Result<Vec<Told>> {
    super::change(&at.common, |q| q.failed = None)?;
    let queue = super::read(&at.common);
    let mut tell = match &queue.train {
        Some(train) => ride(at, forge, &queue, train, now).await?,
        None if queue.held.is_some() || queue.prs.is_empty() => Vec::new(),
        None => {
            let mut tell = Vec::new();
            start(at, forge, &queue, None, now, &mut tell).await?;
            tell
        }
    };
    // What the train under test did is told even when the train ahead
    // fails after it, its failure kept for the Landing view.
    if let Err(e) = ahead_step(at, forge, now, &mut tell).await {
        super::change(&at.common, |q| q.failed = Some(e.to_string()))?;
    }
    Ok(tell)
}

/// The train ahead's part of a step: ended when it no longer stands,
/// built when there is none.
async fn ahead_step(at: &Repo, forge: &impl Forge, now: i64, tell: &mut Vec<Told>) -> Result<()> {
    let queue = super::read(&at.common);
    if let Some(ahead) = &queue.ahead {
        // It stands only on the train under test it was built on, and only
        // while every PR in it is still queued.
        let on_its_train = queue.train.as_ref().is_some_and(|t| t.commit == ahead.base);
        let taken_out = ahead.prs.iter().any(|t| queue.queued(t.number).is_none());
        if !on_its_train || taken_out {
            end_ahead(at, forge, &queue, ahead, tell).await?;
        }
    }
    let queue = super::read(&at.common);
    if let (Some(train), None, None) = (&queue.train, &queue.ahead, &queue.held) {
        start(at, forge, &queue, Some(train), now, tell).await?;
    }
    Ok(())
}

/// Ends the train ahead before it was tested on its own: its run
/// cancelled, its branch deleted, and its PRs back in line, a split's
/// behind the splits there are.
async fn end_ahead(
    at: &Repo,
    forge: &impl Forge,
    queue: &Queue,
    ahead: &Train,
    tell: &mut Vec<Told>,
) -> Result<()> {
    // A run left going would hold a runner for nothing; one that can't be
    // cancelled ends by itself.
    let _ = forge.cancel(&ahead.commit).await;
    delete(&at.common, &ahead.branch);
    super::change(&at.common, |q| {
        q.ahead = None;
        let back: Vec<u64> = ahead
            .prs
            .iter()
            .map(|t| t.number)
            .filter(|n| q.queued(*n).is_some())
            .collect();
        if ahead.split && !back.is_empty() {
            q.splits.push(back);
        }
    })?;
    for tested in &ahead.prs {
        if queue.queued(tested.number).is_some() {
            let words = "back in line: the train ahead it was in ended before it was tested".into();
            tell.push(told(queue, tested.number, words));
        }
    }
    Ok(())
}

/// A PR's entry as `Told`, when it is still queued.
fn told(queue: &Queue, number: u64, words: String) -> Told {
    let entry = queue.queued(number);
    Told {
        number,
        path: entry.map(|e| e.path.clone()).unwrap_or_default(),
        pane: entry.and_then(|e| e.pane.clone()),
        words,
    }
}

/// Drops a PR, telling its tab why.
fn drop_pr(at: &Repo, number: u64, why: String, now: i64, tell: &mut Vec<Told>) -> Result<()> {
    let gone = super::change(&at.common, |q| q.drop_pr(number, &why, now))?;
    if let Some(gone) = gone {
        tell.push(Told {
            number,
            path: gone.path,
            pane: gone.pane,
            words: format!("dropped from the merge queue: {why}"),
        });
    }
    Ok(())
}

/// Builds the next train and pushes it: on the default branch as origin has
/// it, or, given the train under test (`on`), on that train's commit, as
/// the train ahead.
async fn start(
    at: &Repo,
    forge: &impl Forge,
    queue: &Queue,
    on: Option<&Train>,
    now: i64,
    // Pushed to as it goes, so what was told survives an error after it.
    tell: &mut Vec<Told>,
) -> Result<()> {
    let split = queue.splits.first().cloned();
    let group: Vec<u64> = match &split {
        Some(split) => split.clone(),
        // Every PR in order, so one that isn't ready doesn't hold a place.
        None => queue.prs.iter().map(|p| p.number).collect(),
    };
    let group: Vec<u64> = group.into_iter().filter(|n| !queue.testing(*n)).collect();
    if group.is_empty() {
        return Ok(());
    }
    // The split is taken out of the queue only once it is tested, or found
    // to have nothing ready, so a step that fails on the way (a request, a
    // push) tries the same split again.
    let consume = |q: &mut Queue| {
        if let (Some(split), Some(first)) = (&split, q.splits.first())
            && first.iter().all(|n| split.contains(n))
        {
            q.splits.remove(0);
        }
    };
    // A PR is judged by the checks its base requires, as GitHub merges it.
    let required = forge.required(&at.branch).await?;
    let mut ready: Vec<(u64, String)> = Vec::new();
    for number in group {
        if ready.len() >= TRAIN_MAX {
            break;
        }
        // One PR GitHub can't answer for passes over that PR alone, and one
        // it has no such PR for is dropped, so neither stops the queue.
        let pr = match forge.pr(number).await {
            Ok(pr) => pr,
            Err(e)
                if matches!(
                    crate::net::kind_of(&e),
                    Some(crate::net::ErrorKind::NotFound)
                ) =>
            {
                let why = "GitHub has no such pull request".to_string();
                drop_pr(at, number, why, now, tell)?;
                continue;
            }
            // A split waits for an answer on every PR in it, so it isn't
            // taken as tested without one.
            Err(e) if split.is_some() => return Err(e),
            Err(_) => continue,
        };
        match pr.state {
            PrState::Merged => {
                super::change(&at.common, |q| q.merged_pr(number, now))?;
                continue;
            }
            PrState::Closed => {
                drop_pr(at, number, "it was closed".into(), now, tell)?;
                continue;
            }
            PrState::Open => {}
        }
        if pr.base != at.branch {
            let why = format!("it targets {}, not {}", pr.base, at.branch);
            drop_pr(at, number, why, now, tell)?;
            continue;
        }
        // A push to a branch of origin runs the workflows the pushed commit
        // carries, with the repository's secrets, so a train holds only
        // commits of origin's own branches, each the head that was queued.
        if !pr.from_here {
            let why = "its head is a fork's branch, and a train runs only this repository's".into();
            drop_pr(at, number, why, now, tell)?;
            continue;
        }
        if queue.queued(number).map(|q| q.head.as_str()) != Some(pr.head.as_str()) {
            // Pushed to since it was queued: it waits until it is queued
            // again at its new head.
            continue;
        }
        let seen = match forge.checks(&pr.head, required.as_deref()).await {
            Ok(seen) => seen,
            Err(e) if split.is_some() => return Err(e),
            Err(_) => continue,
        };
        match seen {
            Seen::Passed => ready.push((number, pr.head)),
            Seen::Failed { names, .. } => {
                let why = format!("its own checks failed: {}", names.join(", "));
                drop_pr(at, number, why, now, tell)?;
            }
            // A repository with no CI on its PRs (and none required): ready
            // once none has come for `NO_CHECKS`, as wait-ci answers `none`.
            Seen::Nothing
                if required.is_none()
                    && queue
                        .queued(number)
                        .is_some_and(|q| now - q.since > NO_CHECKS) =>
            {
                ready.push((number, pr.head))
            }
            Seen::Running | Seen::Nothing => {}
        }
    }
    if ready.is_empty() {
        super::change(&at.common, consume)?;
        return Ok(());
    }
    // The train ahead's base is the train under test's branch, fetched so
    // its commit is there whatever happened to the clone since it was built.
    let (base_ref, base_branch) = match on {
        Some(train) => (format!("{REFS}/train"), train.branch.as_str()),
        None => (format!("{REFS}/base"), at.branch.as_str()),
    };
    let mut specs = vec![format!("+refs/heads/{base_branch}:{base_ref}")];
    // The train ahead also needs the default branch, to tell a PR that
    // conflicts with it from one that conflicts only with the train under
    // test, which isn't decided yet.
    let main_ref = format!("{REFS}/base");
    if on.is_some() {
        specs.push(format!("+refs/heads/{}:{main_ref}", at.branch));
    }
    specs.extend(
        ready
            .iter()
            .map(|(n, _)| format!("+refs/pull/{n}/head:{REFS}/pr-{n}")),
    );
    fetch(&at.common, &specs)?;
    let base = git(&at.common, &["rev-parse", &base_ref])?;
    if on.is_some_and(|t| t.commit != base) {
        return Err(anyhow!("{base_branch} isn't the train under test's commit"));
    }
    let mut tip = base.clone();
    let mut prs: Vec<Tested> = Vec::new();
    // A split's PRs that wait on the train under test stay a split, so a
    // suspect from a red train is still tested apart from the line.
    let mut waiting: Vec<u64> = Vec::new();
    for (number, head) in ready {
        let fetched = git(&at.common, &["rev-parse", &format!("{REFS}/pr-{number}")])?;
        if fetched != head {
            // Pushed to between GitHub's answer and the fetch: the next
            // train takes it once its checks pass.
            continue;
        }
        match merged_onto(&at.common, &tip, &head, number, on.is_some())? {
            Some(commit) => {
                tip = commit;
                prs.push(Tested { number, head });
            }
            None if on.is_some() => {
                // On top of a train not yet decided, only a conflict with
                // the default branch itself drops it; one with the PRs
                // ahead waits until they merge or fail.
                let main = git(&at.common, &["rev-parse", &main_ref])?;
                if merged_onto(&at.common, &main, &head, number, true)?.is_none() {
                    let why = format!("it doesn't merge cleanly onto {}", at.branch);
                    drop_pr(at, number, why, now, tell)?;
                } else {
                    waiting.push(number);
                }
            }
            None => {
                let why = match prs.as_slice() {
                    [] => format!("it doesn't merge cleanly onto {}", at.branch),
                    ahead => format!(
                        "it doesn't merge cleanly onto {} with {} ahead of it",
                        at.branch,
                        numbered(&ahead.iter().map(|t| t.number).collect::<Vec<_>>())
                    ),
                };
                drop_pr(at, number, why, now, tell)?;
            }
        }
    }
    let keep_waiting = |q: &mut Queue| {
        if split.is_some() && !waiting.is_empty() {
            q.splits.insert(0, waiting.clone());
        }
    };
    if prs.is_empty() {
        super::change(&at.common, |q| {
            consume(q);
            keep_waiting(q);
        })?;
        return Ok(());
    }
    let branch = format!("ci/train-{}", &tip[..12.min(tip.len())]);
    if let Err(e) = push(&at.common, &format!("{tip}:refs/heads/{branch}")) {
        // Tried again on the next step, as a network blip would want.
        super::change(&at.common, |q| {
            q.push_failures += 1;
            if q.push_failures >= super::TRIES {
                q.held = Some(format!("couldn't push {branch}: {e}"));
                q.push_failures = 0;
            }
        })?;
        return Ok(());
    }
    let words = match on {
        Some(under) => format!(
            "testing next on {branch}, on top of {}",
            numbered(&under.prs.iter().map(|t| t.number).collect::<Vec<_>>())
        ),
        None => format!("testing on {branch}"),
    };
    for tested in &prs {
        tell.push(told(queue, tested.number, words.clone()));
    }
    let train = Train {
        branch,
        base,
        commit: tip,
        prs,
        since: now,
        split: split.is_some(),
        ..Train::default()
    };
    super::change(&at.common, |q| {
        match on {
            Some(_) => q.ahead = Some(train),
            None => q.train = Some(train),
        }
        q.push_failures = 0;
        consume(q);
        keep_waiting(q);
    })?;
    Ok(())
}

/// PR numbers as `#1, #2`.
fn numbered(numbers: &[u64]) -> String {
    numbers
        .iter()
        .map(|n| format!("#{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

async fn ride(
    at: &Repo,
    forge: &impl Forge,
    queue: &Queue,
    train: &Train,
    now: i64,
) -> Result<Vec<Told>> {
    let mut tell = Vec::new();
    let end = |held: Option<String>| -> Result<()> {
        delete(&at.common, &train.branch);
        super::change(&at.common, |q| {
            q.train = None;
            if held.is_some() {
                q.held = held;
            }
        })?;
        Ok(())
    };
    // One merged by an earlier try of this step is out of the queue too.
    let taken_out =
        |n: u64| queue.queued(n).is_none() && !queue.merged.iter().any(|m| m.number == n);
    if train.prs.iter().any(|t| taken_out(t.number)) {
        end(None)?;
        return Ok(tell);
    }
    let required = forge.required(&at.branch).await?;
    let mut seen = forge.checks(&train.commit, required.as_deref()).await?;
    // A failure while the run run again hasn't ended is the first
    // attempt's, still showing until GitHub makes the rerun's check runs
    // (the job that failed, known without asking, or another that failed
    // beside it), or one the rerun's run will end on anyway.
    if let (Seen::Failed { job, .. }, Some(rerun)) = (&seen, &train.rerun) {
        let same_job = job.is_some() && *job == rerun.job;
        if same_job || !forge.run_finished(&rerun.run).await? {
            seen = Seen::Running;
        }
    }
    if train.passed && seen != Seen::Passed {
        super::change(&at.common, |q| {
            if let Some(t) = q.train.as_mut() {
                t.passed = false;
            }
        })?;
    }
    match seen {
        Seen::Passed if !train.passed => {
            super::change(&at.common, |q| {
                if let Some(t) = q.train.as_mut() {
                    t.passed = true;
                }
            })?;
        }
        Seen::Nothing if now - train.since > GRACE => {
            end(Some(format!(
                "no CI ran on {} in {} minutes: does the repository's CI run on a push to ci/train-*?",
                train.branch,
                GRACE / 60_000
            )))?;
        }
        Seen::Running if now - train.since > TIMEOUT => {
            end(Some(format!(
                "CI on {} ran over {} minutes",
                train.branch,
                TIMEOUT / 60_000
            )))?;
        }
        Seen::Nothing | Seen::Running => {}
        Seen::Failed { run, names, job } => {
            let requeued = |t: &Tested| queue.queued(t.number).is_some_and(|q| q.head != t.head);
            let requeued_alone = matches!(train.prs.as_slice(), [only] if requeued(only));
            let link = match run.as_str() {
                "-" => String::new(),
                run => format!(" ({})", forge.run_url(run)),
            };
            // Once, and only for a GitHub Actions run, which can be run
            // again; a request that fails drops or splits as below.
            let can_rerun = train.rerun.is_none() && !requeued_alone && run != "-";
            // GitHub runs a run's failed jobs again only once the run has
            // ended, so the train waits for its other jobs first, up to
            // `TIMEOUT` as any run does.
            if can_rerun && now - train.since <= TIMEOUT && !forge.run_finished(&run).await? {
                return Ok(tell);
            }
            if can_rerun && forge.rerun(&run).await.is_ok() {
                let rerun = Rerun {
                    run: run.clone(),
                    names: names.clone(),
                    job,
                };
                super::change(&at.common, |q| {
                    if let Some(t) = q.train.as_mut() {
                        t.rerun = Some(rerun);
                        t.passed = false;
                        // The rerun has `TIMEOUT` of its own.
                        t.since = now;
                    }
                })?;
                for tested in &train.prs {
                    let words = format!(
                        "{} failed {}{link}: running its failed jobs again",
                        train.branch,
                        names.join(", ")
                    );
                    tell.push(told(queue, tested.number, words));
                }
                return Ok(tell);
            }
            delete(&at.common, &train.branch);
            if requeued_alone {
                // Queued again at a new head while its train ran: the
                // failure is the old head's, so it waits for a train of
                // its own.
                super::change(&at.common, |q| q.train = None)?;
            } else if let [only] = train.prs.as_slice() {
                let why = format!(
                    "CI failed on its train {}: {}{link}",
                    train.branch,
                    names.join(", ")
                );
                super::change(&at.common, |q| q.train = None)?;
                drop_pr(at, only.number, why, now, &mut tell)?;
            } else {
                let numbers: Vec<u64> = train.prs.iter().map(|t| t.number).collect();
                let (first, second) = numbers.split_at(numbers.len() / 2);
                super::change(&at.common, |q| {
                    q.train = None;
                    q.splits.insert(0, second.to_vec());
                    q.splits.insert(0, first.to_vec());
                })?;
            }
        }
        Seen::Passed => {
            fetch(
                &at.common,
                &[format!("+refs/heads/{}:{REFS}/base", at.branch)],
            )?;
            let base_now = git(&at.common, &["rev-parse", &format!("{REFS}/base")])?;
            // Once one of its PRs merged, the default branch is where that
            // merge left it.
            let expected = train.merged_base.as_deref().unwrap_or(&train.base);
            if base_now != expected {
                // Something else landed: what the train tested isn't what
                // merging it would leave, so it is built and tested again.
                end(None)?;
                return Ok(tell);
            }
            let mut all_merged = true;
            for tested in &train.prs {
                let number = tested.number;
                let pr = forge.pr(number).await?;
                if pr.state == PrState::Merged {
                    super::change(&at.common, |q| q.merged_pr(number, now))?;
                    continue;
                }
                if pr.state == PrState::Closed {
                    drop_pr(at, number, "it was closed".into(), now, &mut tell)?;
                    all_merged = false;
                    break;
                }
                if pr.head != tested.head {
                    tell.push(told(
                        queue,
                        number,
                        "its head moved during its train, so it waits until it is queued again at its new head".into(),
                    ));
                    all_merged = false;
                    break;
                }
                let merged = forge.merge(number, &tested.head).await;
                if let Err(MergeError::Strict(why)) = merged {
                    end(Some(format!(
                        "{} requires branches to be up to date, which a queued PR never is: turn that off in its branch protection ({why})",
                        at.branch
                    )))?;
                    return Ok(tell);
                } else if let Err(MergeError::Refused(why)) = merged {
                    // The rest were tested with it: they go in a train again.
                    end(None)?;
                    let why = format!("GitHub won't merge it: {why}");
                    drop_pr(at, number, why, now, &mut tell)?;
                    return Ok(tell);
                } else if let Err(e) = merged {
                    if train.merge_failures + 1 >= super::TRIES {
                        end(Some(format!("GitHub refused to merge #{number}: {e}")))?;
                    } else {
                        // Tried again on the next step, the train kept.
                        super::change(&at.common, |q| {
                            if let Some(t) = q.train.as_mut() {
                                t.merge_failures += 1;
                            }
                        })?;
                    }
                    return Ok(tell);
                }
                fetch(
                    &at.common,
                    &[format!("+refs/heads/{}:{REFS}/base", at.branch)],
                )?;
                let left = git(&at.common, &["rev-parse", &format!("{REFS}/base")])?;
                let gone = super::change(&at.common, |q| {
                    if let Some(t) = q.train.as_mut() {
                        t.merged_base = Some(left);
                        t.merge_failures = 0;
                    }
                    q.merged_pr(number, now)
                })?;
                tell.push(Told {
                    number,
                    path: gone.as_ref().map(|g| g.path.clone()).unwrap_or_default(),
                    pane: gone.and_then(|g| g.pane),
                    words: format!("merged by the merge queue from {}", train.branch),
                });
            }
            let promoted = match &queue.ahead {
                // One of its PRs taken out ends it instead, in the train
                // ahead's part of the step.
                Some(ahead)
                    if all_merged
                        && ahead.base == train.commit
                        && ahead
                            .prs
                            .iter()
                            .all(|t| super::read(&at.common).queued(t.number).is_some()) =>
                {
                    // A check that fails (a fetch) promotes nothing: the train
                    // ahead is built again, and what merged is still told.
                    promote(at, train).await.unwrap_or(None)
                }
                _ => None,
            };
            if let Some(base) = promoted {
                // The train ahead becomes the train under test, its CI
                // counting as it stands: its base is now what the default
                // branch holds.
                delete(&at.common, &train.branch);
                super::change(&at.common, |q| {
                    q.train = q.ahead.take().map(|ahead| Train { base, ..ahead });
                })?;
                if let Some(ahead) = &queue.ahead {
                    for tested in &ahead.prs {
                        let words = format!("testing on {}", ahead.branch);
                        tell.push(told(queue, tested.number, words));
                    }
                }
            } else {
                end(None)?;
            }
        }
    }
    Ok(tell)
}

/// Where the default branch is once a train's PRs all merged, when it holds
/// the tree the train tested, so the train ahead, built on that train's
/// commit, tested what merging it will leave; `None` otherwise, which
/// rebuilds it.
async fn promote(at: &Repo, train: &Train) -> Result<Option<String>> {
    fetch(
        &at.common,
        &[format!("+refs/heads/{}:{REFS}/base", at.branch)],
    )?;
    let base = git(&at.common, &["rev-parse", &format!("{REFS}/base")])?;
    let tree = |commit: &str| git(&at.common, &["rev-parse", &format!("{commit}^{{tree}}")]);
    Ok((tree(&base)? == tree(&train.commit)?).then_some(base))
}

/// Fetches from origin, counted in the request audit.
fn fetch(common: &Path, specs: &[String]) -> Result<()> {
    crate::requests::count("github", "git", SOURCE);
    let mut args = vec!["fetch", "--quiet", "--no-tags", "origin"];
    args.extend(specs.iter().map(String::as_str));
    git(common, &args).map(|_| ())
}

fn push(common: &Path, spec: &str) -> Result<()> {
    crate::requests::count("github", "git", SOURCE);
    git(common, &["push", "--quiet", "origin", spec]).map(|_| ())
}

/// Deletes a train's branch on origin, quietly: one already gone is fine.
fn delete(common: &Path, branch: &str) {
    crate::requests::count("github", "git", SOURCE);
    let _ = git(common, &["push", "--quiet", "origin", "--delete", branch]);
}

/// `head` merged onto `tip`, a commit with both as parents, or `None` when
/// they conflict. The train ahead's say so, so the halves of a red train,
/// the second built on the first, are never that train's commit again,
/// whose failed runs GitHub would show on it.
fn merged_onto(
    common: &Path,
    tip: &str,
    head: &str,
    number: u64,
    ahead: bool,
) -> Result<Option<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(common)
        .args(["merge-tree", "--write-tree", "--no-messages", tip, head])
        .stdin(std::process::Stdio::null())
        .output()?;
    match out.status.code() {
        Some(0) => {}
        Some(1) => return Ok(None),
        _ => {
            return Err(anyhow!(
                "git merge-tree failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
    }
    let tree = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let message = match ahead {
        true => format!("Merge #{number} into the merge queue's train ahead"),
        false => format!("Merge #{number} into the merge queue's train"),
    };
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(common)
        .args(["commit-tree", &tree, "-p", tip, "-p", head, "-m", &message])
        // A train's commits are the queue's, pushed only to its branch.
        .env("GIT_AUTHOR_NAME", "Switchboard merge queue")
        .env("GIT_AUTHOR_EMAIL", "merge-queue@switchboard.invalid")
        .env("GIT_COMMITTER_NAME", "Switchboard merge queue")
        .env("GIT_COMMITTER_EMAIL", "merge-queue@switchboard.invalid");
    Ok(Some(util::run_command(&mut cmd)?.trim().to_string()))
}

#[cfg(test)]
mod tests;
