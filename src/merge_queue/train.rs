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
//! moved, the train is tested again. Red, a train of one drops its PR, and a
//! longer one is split into halves tested first, until the PR at fault is
//! the only one dropped. A PR taken out of the queue ends its train. A
//! train with no CI after `GRACE`, or none finished after `TIMEOUT`, holds
//! the queue until something in it changes, and so do a push of a train or
//! a merge failing `TRIES` times in a row, tried again on each step until
//! then. Every way a train ends deletes its branch.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, anyhow};

use super::{Queue, TRAIN_MAX, Tested, Train};
use crate::cmd::wait_ci::Seen;
use crate::util;

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
    let queue = super::read(&at.common);
    if let Some(train) = &queue.train {
        return ride(at, forge, &queue, train, now).await;
    }
    if queue.held.is_some() || queue.prs.is_empty() {
        return Ok(Vec::new());
    }
    start(at, forge, &queue, now).await
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

async fn start(at: &Repo, forge: &impl Forge, queue: &Queue, now: i64) -> Result<Vec<Told>> {
    let mut tell = Vec::new();
    let split = queue.splits.first().cloned();
    let group: Vec<u64> = match &split {
        Some(split) => split.clone(),
        // Every PR in order, so one that isn't ready doesn't hold a place.
        None => queue.prs.iter().map(|p| p.number).collect(),
    };
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
                drop_pr(at, number, why, now, &mut tell)?;
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
                drop_pr(at, number, "it was closed".into(), now, &mut tell)?;
                continue;
            }
            PrState::Open => {}
        }
        if pr.base != at.branch {
            let why = format!("it targets {}, not {}", pr.base, at.branch);
            drop_pr(at, number, why, now, &mut tell)?;
            continue;
        }
        // A push to a branch of origin runs the workflows the pushed commit
        // carries, with the repository's secrets, so a train holds only
        // commits of origin's own branches, each the head that was queued.
        if !pr.from_here {
            let why = "its head is a fork's branch, and a train runs only this repository's".into();
            drop_pr(at, number, why, now, &mut tell)?;
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
                drop_pr(at, number, why, now, &mut tell)?;
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
        return Ok(tell);
    }
    let base_ref = format!("{REFS}/base");
    let mut specs = vec![format!("+refs/heads/{}:{base_ref}", at.branch)];
    specs.extend(
        ready
            .iter()
            .map(|(n, _)| format!("+refs/pull/{n}/head:{REFS}/pr-{n}")),
    );
    fetch(&at.common, &specs)?;
    let base = git(&at.common, &["rev-parse", &base_ref])?;
    let mut tip = base.clone();
    let mut prs: Vec<Tested> = Vec::new();
    for (number, head) in ready {
        let fetched = git(&at.common, &["rev-parse", &format!("{REFS}/pr-{number}")])?;
        if fetched != head {
            // Pushed to between GitHub's answer and the fetch: the next
            // train takes it once its checks pass.
            continue;
        }
        match merged_onto(&at.common, &tip, &head, number)? {
            Some(commit) => {
                tip = commit;
                prs.push(Tested { number, head });
            }
            None => {
                let why = match prs.as_slice() {
                    [] => format!("it doesn't merge cleanly onto {}", at.branch),
                    ahead => format!(
                        "it doesn't merge cleanly onto {} with {} ahead of it",
                        at.branch,
                        ahead
                            .iter()
                            .map(|t| format!("#{}", t.number))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                };
                drop_pr(at, number, why, now, &mut tell)?;
            }
        }
    }
    if prs.is_empty() {
        super::change(&at.common, consume)?;
        return Ok(tell);
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
        return Ok(tell);
    }
    let train = Train {
        branch,
        base,
        commit: tip,
        prs,
        since: now,
        ..Train::default()
    };
    super::change(&at.common, |q| {
        q.train = Some(train);
        q.push_failures = 0;
        consume(q);
    })?;
    Ok(tell)
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
    let seen = forge.checks(&train.commit, required.as_deref()).await?;
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
        Seen::Failed { run, names, .. } => {
            delete(&at.common, &train.branch);
            let requeued = |t: &Tested| queue.queued(t.number).is_some_and(|q| q.head != t.head);
            if let [only] = train.prs.as_slice()
                && requeued(only)
            {
                // Queued again at a new head while its train ran: the
                // failure is the old head's, so it waits for a train of
                // its own.
                super::change(&at.common, |q| q.train = None)?;
            } else if let [only] = train.prs.as_slice() {
                let link = match run.as_str() {
                    "-" => String::new(),
                    run => format!(" ({})", forge.run_url(run)),
                };
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
            for tested in &train.prs {
                let number = tested.number;
                let pr = forge.pr(number).await?;
                if pr.state == PrState::Merged {
                    super::change(&at.common, |q| q.merged_pr(number, now))?;
                    continue;
                }
                if pr.state == PrState::Closed {
                    drop_pr(at, number, "it was closed".into(), now, &mut tell)?;
                    break;
                }
                if pr.head != tested.head {
                    tell.push(told(
                        queue,
                        number,
                        "its head moved during its train, so it waits until it is queued again at its new head".into(),
                    ));
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
            end(None)?;
        }
    }
    Ok(tell)
}

fn git(common: &Path, args: &[&str]) -> Result<String> {
    crate::worktree::git_at(common, args)
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
/// they conflict.
fn merged_onto(common: &Path, tip: &str, head: &str, number: u64) -> Result<Option<String>> {
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
    let message = format!("Merge #{number} into the merge queue's train");
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
