//! `wait-ci`: waits until a commit's CI has finished, and prints one line.
//! queue-and-clean runs it for a landing whose tests run in CI (repos.json's
//! `testIn` `ci`), after pushing the landing's commit to a branch of its own,
//! so the tests run on GitHub's computers rather than on this one. It is new
//! with the Rust version: the tmux version's landings ran their tests in the
//! worktree alone.
//!
//! ```text
//! passed <sha>                        every check run finished, none failing, one at least succeeding
//! failed <sha> <run id> <job names>   a check run finished failing (run id "-" when it has none)
//! none                                no check run that ran anything within --grace minutes
//! timeout                             nothing settled within --timeout minutes
//!
//! wait-ci --repo <owner/name> --sha <commit> [--timeout 45] [--grace 3]
//! wait-ci --now --repo <owner/name> --sha <commit>
//! wait-ci [--now] --repo <owner/name> --sha <commit> --same-as <base commit>
//! ```
//!
//! `--same-as` sets a failure beside the default branch's run at `<base
//! commit>` and says, in place of `failed`, whose it is: `base-failing`
//! when every test the landing's failed jobs name failed there too, in the
//! same job or another partition of it (a test moves between partitions as
//! tests are added; another platform's job doesn't count), `change-failing`
//! when none did (a test that passed there, or a job naming no test
//! whose whole family passed there), and `mixed` for some of each or what
//! can't be told: a job naming no test whose family failed there (one that
//! failed building, or linting), a job missing, skipped or still running
//! there, a failed job there whose tests can't be read, a log with an error
//! besides its tests' (a build or lint error), two jobs of the same name
//! (in two workflows), a log that can't be read, or a base GitHub doesn't
//! answer for. `base-failing` waits for the landing's other jobs to finish,
//! since one yet to fail may be the change's (with `--now`, it is `mixed`
//! while they run). The tests are read from cargo's and nextest's lines in
//! each failed job's log (`failing_tests`), by their path, so a landing
//! waits on the default branch only for failures it can show are its, and
//! reads the logs itself only on `mixed`.
//!
//! `--now` asks once and says what that one look sees, with no waiting:
//! `passed <sha>`, `failed <sha> <run id> <job names>`, `running <sha>` or
//! `none`. queue-and-clean asks it whether the default branch's CI is
//! failing before it checks a clean rebase with repos.json's `afterRebase`
//! rather than testing it again, and a pass there needs one look, since a
//! job still to come only delays a pass rather than turning it into one.
//! The look (`look`) is the server's too, for the default branch's CI it
//! watches (src/sources/mainci.rs).
//!
//! It asks GitHub for the commit's check runs, which GitHub Actions makes
//! one per job, and its commit statuses, which some other CIs report
//! instead, each status read as a check run by its context. A failure is said as soon as
//! one job has failed, with the others still running, since the landing
//! fixes it and pushes again, which makes the run moot; the run's id is read
//! from the check run's link, for `gh run view`. Passing has to hold on two
//! looks in a row, since a job that waits on another gets its check run only
//! once that one ends, and for a moment every check run there is has passed.
//! `none` is for a repository with no CI, or one whose CI doesn't run on the
//! branch pushed, its jobs skipped there included (check runs all skipped or
//! neutral ran nothing, so they are no evidence the tests passed): the
//! landing then runs its tests in the worktree. A failed
//! request is retried on the next round rather than read as an answer, and
//! never as `none`; one GitHub refuses (no token, or one it won't take) ends
//! it with the reason on stderr and exit 1, as `wait-deploy` does.
//!
//! GitHub is asked over its REST API (src/integrations/github.rs), which
//! counts each request, every 30 seconds, or every `WAIT_CI_POLL_SECS`, for
//! the tests.

use std::collections::{BTreeMap, BTreeSet};
use std::process::ExitCode;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::Value;

use super::wait_deploy::refused;
use crate::integrations::github;
use crate::sources::ciboard;
use crate::util;

const POLL: Duration = Duration::from_secs(30);
const SOURCE: &str = "wait-ci";
const USAGE: &str = "usage: wait-ci [--now] --repo <owner/name> --sha <commit> [--same-as <commit>] [--timeout 45] [--grace 3]";

/// Whose a landing's failure is, set beside the default branch's run
/// (`--same-as`), said in place of `failed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blame {
    /// Every failure is one the default branch's run has too.
    Base,
    /// None of them is.
    Change,
    /// Some are and some aren't, or one can't be told: the logs are read.
    Mixed,
}

impl Blame {
    fn word(self) -> &'static str {
        match self {
            Blame::Base => "base-failing",
            Blame::Change => "change-failing",
            Blame::Mixed => "mixed",
        }
    }
}

/// One failure of the landing's run, set beside the default branch's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Base,
    Change,
    Unknown,
}

fn blame(parts: &[Part]) -> Blame {
    if !parts.is_empty() && parts.iter().all(|p| *p == Part::Base) {
        Blame::Base
    } else if !parts.is_empty() && parts.iter().all(|p| *p == Part::Change) {
        Blame::Change
    } else {
        Blame::Mixed
    }
}

/// The tests a job's log says failed, by their path: libtest's `test <path>
/// ... FAILED` and nextest's `FAIL [ 0.1s] <binary> <path>` (or `TIMEOUT`,
/// a signal, a leak), its log read without colors and timestamps. None
/// when the log has an error that isn't a test's (a build or lint error,
/// `error[E0425]: …`, `error: could not compile …`, an Actions error
/// other than the failed step's exit code, a second step failing, or a
/// cargo test binary that crashed, exiting otherwise than with 101),
/// whose failure the tests don't account for.
fn failing_tests(log: &str) -> Option<BTreeSet<String>> {
    static CARGO: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^test ([^\n]+?)(?: - should panic)? \.\.\. FAILED\s*$").unwrap()
    });
    static NEXTEST: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?m)^\s*(?:TRY \d+ )?(?:FAIL|TIMEOUT|SIG[A-Z]+|ABORT|LEAK-FAIL)\s*\[[^\]]*\]\s+(?:\(\s*\d+/\s*\d+\)\s+)?\S+\s+(\S+)\s*$",
        )
        .unwrap()
    });
    // cargo's and nextest's own words for tests that failed, which every
    // failing test run ends with.
    static TESTS_FAILED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?:test failed|doctest failed|test run failed|\d+ targets? failed|process didn't exit successfully)").unwrap()
    });
    static ERROR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^error(?:\[E\d+\])?: (.*)$").unwrap());
    // A test nextest retried that passed at last (`TRY 2 PASS`, and
    // `FLAKY` in the summary) didn't fail the run; a plain `PASS` is
    // another run of it, which says nothing of this one.
    static PASSED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^\s*(?:TRY \d+ PASS|FLAKY)\b[^\[]*\[\s*[\d.]+s\]\s+(?:\(\s*\d+/\s*\d+\)\s+)?\S+\s+(\S+)\s*$").unwrap()
    });
    let log = ciboard::readable(log);
    let passed: BTreeSet<&str> = PASSED
        .captures_iter(&log)
        .map(|c| c.get(1).map_or("", |m| m.as_str()))
        .collect();
    // GitHub Actions' own errors (`##[error]`, read as `Error: `): the one
    // ending the step that failed is the tests', and any other (a second
    // step failing, a lint's annotation) is not.
    static ACTIONS_ERROR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^Error: (.*)$").unwrap());
    let actions_errors: Vec<&str> = ACTIONS_ERROR
        .captures_iter(&log)
        .map(|c| c.get(1).map_or("", |m| m.as_str()))
        .collect();
    let steps_failed = actions_errors
        .iter()
        // cargo test exits 101 for failed tests and nextest 100; any other
        // code (a runner killed for memory, 137) is a failure of its own.
        .filter(|e| {
            matches!(
                e.trim_end(),
                "Process completed with exit code 100." | "Process completed with exit code 101."
            )
        })
        .count();
    // A cargo test binary that crashed (a stack overflow, a signal) exits
    // otherwise than libtest's 101 for failed tests, which cargo says.
    let crashed = log
        .lines()
        .filter(|l| l.contains("process didn't exit successfully"))
        .any(|l| !l.contains("(exit status: 101)"));
    // A failure line the patterns above don't read is a failure they
    // can't account for.
    static NEXTEST_FAILURE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^\s*(?:TRY \d+ )?(?:FAIL|TIMEOUT|SIG[A-Z]+|ABORT|LEAK-FAIL)\s*\[").unwrap()
    });
    let unread = log.lines().filter(|l| l.contains(" ... FAILED")).count()
        > CARGO.captures_iter(&log).count()
        || NEXTEST_FAILURE.find_iter(&log).count() > NEXTEST.captures_iter(&log).count();
    let other_error = crashed
        || unread
        || steps_failed > 1
        || steps_failed < actions_errors.len()
        || ERROR
            .captures_iter(&log)
            .any(|c| !TESTS_FAILED.is_match(&c[1]));
    (!other_error).then(|| {
        CARGO
            .captures_iter(&log)
            .chain(NEXTEST.captures_iter(&log))
            .map(|c| c[1].to_string())
            .filter(|test| !passed.contains(test.as_str()))
            .collect()
    })
}

/// A landing's failed job, by the tests its log says failed, beside what
/// the default branch's run did with them in a job of its family (a test
/// moves between partitions as tests are added, `family`): each is the
/// default branch's when that run failed it too, the change's when that
/// run passed it (`base_passing`, from its failed jobs' logs) or every
/// job of the family passed there (`family_passed`), and can't be told
/// otherwise, a test that run may never have reached. A log that can't be
/// read, names no test, or has an error besides the tests' (a job that
/// failed building, or linting) can't be told by its tests.
fn parts_of(
    landing: Option<&BTreeSet<String>>,
    base_failing: &BTreeSet<String>,
    base_passing: &BTreeSet<String>,
    family_passed: bool,
) -> Vec<Part> {
    match landing {
        Some(landing) if !landing.is_empty() => landing
            .iter()
            .map(|test| {
                if base_failing.contains(test) {
                    Part::Base
                } else if family_passed || base_passing.contains(test) {
                    Part::Change
                } else {
                    Part::Unknown
                }
            })
            .collect(),
        _ => vec![Part::Unknown],
    }
}

/// The tests a failed GitHub Actions job's log names, none for a check of
/// A GitHub Actions job's log, none for a check of another CI or a log
/// that can't be read.
async fn log_of(repo: &str, check_run: &Value) -> Option<String> {
    let job = job_of(check_run)?;
    let path = format!("repos/{repo}/actions/jobs/{job}/logs");
    match github::rest("GET", &path, None, SOURCE).await.ok()? {
        Value::String(log) => Some(log),
        _ => None,
    }
}

/// The tests a job's log says passed, by their path as `failing_tests`
/// reads them: libtest's `test <path> ... ok` and nextest's `PASS [ 0.1s]
/// <binary> <path>`. A failed job's log shows which tests it reached, as
/// a run may stop before its last test command.
fn passing_tests(log: &str) -> BTreeSet<String> {
    static CARGO: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^test ([^\n]+?)(?: - should panic)? \.\.\. ok\s*$").unwrap()
    });
    static NEXTEST: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?m)^\s*(?:TRY \d+ )?PASS\s*\[[^\]]*\]\s+(?:\(\s*\d+/\s*\d+\)\s+)?\S+\s+(\S+)\s*$",
        )
        .unwrap()
    });
    let log = ciboard::readable(log);
    CARGO
        .captures_iter(&log)
        .chain(NEXTEST.captures_iter(&log))
        .map(|c| c[1].to_string())
        .collect()
}

/// A job's name with its partition taken out (`Tests (macOS, 1 of 2)`,
/// `Tests (2/2)`), so a platform's partitions pool their failures while
/// its other names (a platform, a feature set) stay apart.
fn family(name: &str) -> String {
    static PARTITION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\d+\s*(?:of|/)\s*\d+").unwrap());
    PARTITION.replace_all(name, "#").into_owned()
}

/// Whose the landing's failed check runs are, by the run at `base`: a
/// failed job's tests are each set beside what that run did with them in
/// a job of its family, its other partitions included (`parts_of`), and a
/// job naming no test, its log read, is the change's when its whole family
/// passed there, and can't be told otherwise. Two jobs of one
/// name can't be paired, and a base GitHub can't be asked about is
/// `Mixed`.
async fn blame_of(repo: &str, landing: &[Value], base: &str) -> Blame {
    let Ok((_, base_runs)) = check_runs(repo, base, SOURCE).await else {
        return Blame::Mixed;
    };
    let mut base_failing: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut base_passing: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    // The families whose run on the default branch didn't pass whole and
    // known: a job of theirs missing, still running, skipped, or failed with
    // tests that can't be read. Another family's jobs (a release job
    // skipped there) don't count.
    let mut unsure: BTreeSet<String> = BTreeSet::new();
    let shared_name =
        |runs: &[Value], name: &str| runs.iter().filter(|c| text(c, "name") == name).count() > 1;
    for job in &base_runs {
        let name = text(job, "name");
        let kin = family(name);
        let ran = text(job, "status") == "completed"
            && !matches!(text(job, "conclusion"), "skipped" | "neutral");
        if !ran {
            unsure.insert(kin);
            continue;
        }
        if !has_failed(job) {
            continue;
        }
        // A job whose name two workflows share can't be told for which
        // family it speaks, so it lends no failures.
        if shared_name(&base_runs, name) {
            unsure.insert(kin);
            continue;
        }
        let log = log_of(repo, job).await;
        match log
            .as_deref()
            .map(|log| (failing_tests(log), passing_tests(log)))
        {
            Some((Some(tests), passing)) if !tests.is_empty() => {
                base_passing.entry(kin.clone()).or_default().extend(passing);
                base_failing.entry(kin).or_default().extend(tests);
            }
            _ => {
                unsure.insert(kin);
            }
        }
    }
    // A family the landing has a job of that the default branch's run
    // hasn't (a partition not made there yet) isn't known to have run.
    for job in landing {
        let name = text(job, "name");
        if !base_runs.iter().any(|b| text(b, "name") == name) {
            unsure.insert(family(name));
        }
    }
    let mut parts = Vec::new();
    for failed in landing.iter().filter(|c| has_failed(c)) {
        let name = text(failed, "name");
        if shared_name(landing, name) || shared_name(&base_runs, name) {
            parts.push(Part::Unknown);
            continue;
        }
        let counterpart = base_runs.iter().find(|b| text(b, "name") == name);
        let finished = counterpart.is_some_and(|b| text(b, "status") == "completed");
        let passed = finished && counterpart.is_some_and(|b| text(b, "conclusion") == "success");
        // A log read that names no test, or has a failure its tests don't
        // explain (a build or lint error), is the change's when every job
        // of its family passed there, so none of the default branch's
        // failures could have moved into it; one that can't be read can't
        // be told.
        let kin = family(name);
        let family_passed = passed && !unsure.contains(&kin) && !base_failing.contains_key(&kin);
        let Some(log) = log_of(repo, failed).await else {
            parts.push(Part::Unknown);
            continue;
        };
        match failing_tests(&log) {
            Some(tests) if !tests.is_empty() => {
                let none = BTreeSet::new();
                let failing = base_failing.get(&kin).unwrap_or(&none);
                let passing = base_passing.get(&kin).unwrap_or(&none);
                parts.extend(parts_of(Some(&tests), failing, passing, family_passed))
            }
            _ if family_passed => parts.push(Part::Change),
            _ => parts.push(Part::Unknown),
        }
    }
    blame(&parts)
}

/// What a commit's check runs say at one look.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    /// No check run yet, or none that ran anything: all skipped or neutral.
    Nothing,
    /// Some still run, none has failed.
    Running,
    /// Every one has finished, none failing and one at least succeeding.
    Passed,
    /// One has finished failing: its run's id, the failed ones' names, and
    /// the first one's GitHub Actions job, for its log.
    Failed {
        run: String,
        names: Vec<String>,
        job: Option<String>,
    },
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The id of the GitHub Actions run a check run belongs to, from its link
/// (`…/actions/runs/<run>/job/<job>`); a check run of another CI has none.
fn run_of(check_run: &Value) -> Option<String> {
    let (_, rest) = text(check_run, "details_url").split_once("/actions/runs/")?;
    let id: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!id.is_empty()).then_some(id)
}

/// A commit status as the check run it would be: its context the name,
/// `pending` still running, `success` passing and `failure` or `error`
/// failing, with no run of GitHub's.
fn as_check_run(status: &Value) -> Value {
    let state = text(status, "state");
    serde_json::json!({
        "name": text(status, "context"),
        "status": if state == "pending" { "in_progress" } else { "completed" },
        "conclusion": if state == "success" { "success" } else { "failure" },
    })
}

/// The id of the GitHub Actions job a check run is, from its link.
fn job_of(check_run: &Value) -> Option<String> {
    let (_, rest) = text(check_run, "details_url").split_once("/job/")?;
    let id: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!id.is_empty() && run_of(check_run).is_some()).then_some(id)
}

/// A commit's check runs and commit statuses at one look, the statuses
/// read as check runs, with the commit `at` names (a hash, or a branch,
/// for its tip). A request that fails, or one GitHub refuses, is an error
/// (`refused` tells them apart).
pub async fn look(repo: &str, at: &str, source: &str) -> anyhow::Result<(String, Seen)> {
    let (sha, all) = check_runs(repo, at, source).await?;
    Ok((sha, seen(&all)))
}

/// What `look` sees before it is summed up: the commit's hash and its
/// check runs, its statuses read as check runs.
async fn check_runs(repo: &str, at: &str, source: &str) -> anyhow::Result<(String, Vec<Value>)> {
    let checks = format!("repos/{repo}/commits/{at}/check-runs?per_page=100");
    let statuses = format!("repos/{repo}/commits/{at}/status?per_page=100");
    let checks = github::rest("GET", &checks, None, source).await?;
    let statuses = github::rest("GET", &statuses, None, source).await?;
    let list = |answer: &Value, key: &str| {
        answer
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let mut all = list(&checks, "check_runs");
    all.extend(list(&statuses, "statuses").iter().map(as_check_run));
    let sha = statuses
        .get("sha")
        .and_then(Value::as_str)
        .unwrap_or(at)
        .to_string();
    Ok((sha, all))
}

fn has_failed(check_run: &Value) -> bool {
    text(check_run, "status") == "completed"
        && !matches!(
            text(check_run, "conclusion"),
            "success" | "skipped" | "neutral"
        )
}

fn seen(check_runs: &[Value]) -> Seen {
    let failed: Vec<&Value> = check_runs.iter().filter(|c| has_failed(c)).collect();
    if let Some(first) = failed.first() {
        return Seen::Failed {
            run: run_of(first).unwrap_or_else(|| "-".into()),
            names: failed.iter().map(|c| text(c, "name").to_string()).collect(),
            job: failed.iter().find_map(|c| job_of(c)),
        };
    }
    if check_runs.iter().any(|c| text(c, "status") != "completed") {
        Seen::Running
    } else if check_runs
        .iter()
        .any(|c| text(c, "conclusion") == "success")
    {
        Seen::Passed
    } else {
        Seen::Nothing
    }
}

pub fn main(args: Vec<String>) -> ExitCode {
    // A program you run asks the services whatever Settings says.
    crate::integrations::run_by_you();
    let arg = |name: &str| {
        let flag = format!("--{name}");
        args.iter()
            .position(|a| *a == flag)
            .and_then(|at| args.get(at + 1))
            .cloned()
    };
    let number = |given: Option<String>| {
        given
            .and_then(|t| t.trim().parse::<f64>().ok())
            .filter(|t| t.is_finite() && *t >= 0.0)
    };
    let minutes = |name: &str, default: f64| {
        Duration::from_secs_f64(number(arg(name)).unwrap_or(default) * 60.0)
    };
    let (repo, sha) = (
        arg("repo").unwrap_or_default(),
        arg("sha").unwrap_or_default(),
    );
    if repo.is_empty() || sha.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let timeout = minutes("timeout", 45.0);
    let grace = minutes("grace", 3.0);
    let poll = number(std::env::var("WAIT_CI_POLL_SECS").ok())
        .map(Duration::from_secs_f64)
        .unwrap_or(POLL);
    let same_as = arg("same-as").filter(|base| !base.is_empty());
    // A failure's line: `failed`, or with `--same-as` whose it is. A
    // failure that is the base's so far while other jobs of the landing
    // still run is no answer yet when waiting (a job yet to fail may be
    // the change's), and `mixed` at one look.
    let failed = |run: &str, names: &[String], all: &[Value], waiting: bool| {
        let word = match &same_as {
            Some(base) => {
                let still_running = all.iter().any(|c| text(c, "status") != "completed");
                match util::block_on(blame_of(&repo, all, base)) {
                    Blame::Base if still_running && waiting => return None,
                    Blame::Base if still_running => Blame::Mixed.word(),
                    blame => blame.word(),
                }
            }
            None => "failed",
        };
        Some(format!("{word} {sha} {run} {}", names.join(", ")))
    };
    if args.iter().any(|a| a == "--now") {
        return match util::block_on(check_runs(&repo, &sha, SOURCE)) {
            Ok((_, all)) => {
                println!(
                    "{}",
                    match seen(&all) {
                        Seen::Failed { run, names, .. } => {
                            failed(&run, &names, &all, false).unwrap_or_default()
                        }
                        Seen::Passed => format!("passed {sha}"),
                        Seen::Running => format!("running {sha}"),
                        Seen::Nothing => "none".to_string(),
                    }
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("wait-ci: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let started = Instant::now();
    let mut passed_before = false;
    loop {
        let looked = util::block_on(check_runs(&repo, &sha, SOURCE));
        let (now, all) = match looked {
            Ok((_, all)) => (Some(seen(&all)), all),
            // No token, or one GitHub refuses, won't mend by waiting, so
            // it is said at once rather than spending the whole timeout.
            Err(e) if refused(&e) => {
                eprintln!("wait-ci: {e}");
                return ExitCode::FAILURE;
            }
            // A network blip or a rate limit: try again next round.
            Err(_) => (None, Vec::new()),
        };
        let answer = match &now {
            Some(Seen::Failed { run, names, .. }) => match failed(run, names, &all, true) {
                None if started.elapsed() > timeout => Some("timeout".to_string()),
                line => line,
            },
            Some(Seen::Passed) if passed_before => Some(format!("passed {sha}")),
            Some(Seen::Nothing) if started.elapsed() > grace => Some("none".to_string()),
            _ if started.elapsed() > timeout => Some("timeout".to_string()),
            _ => None,
        };
        if let Some(answer) = answer {
            println!("{answer}");
            return ExitCode::SUCCESS;
        }
        passed_before = now == Some(Seen::Passed);
        std::thread::sleep(poll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn check(name: &str, status: &str, conclusion: Value) -> Value {
        json!({
            "name": name,
            "status": status,
            "conclusion": conclusion,
            "details_url": "https://github.com/o/r/actions/runs/77/job/5",
        })
    }

    const CARGO_LOG: &str = "2026-11-15T10:00:00.1234567Z running 3 tests\n\
        2026-11-15T10:00:00.2Z test tabs::names_a_tab ... ok\n\
        2026-11-15T10:00:00.3Z test \x1b[31mmux::keys::binds_m_p ... FAILED\x1b[0m\n\
        2026-11-15T10:00:00.4Z test notes::id::keeps_its_id ... FAILED\n\
        failures:\n    mux::keys::binds_m_p\n";
    const NEXTEST_LOG: &str = "        PASS [   0.012s] switchboard::it tabs::names_a_tab\n\
        \x20       FAIL [   1.204s] (  3/401) switchboard::it wait_ci::same_as\n\
        \x20    TIMEOUT [  60.000s] switchboard::it mux::upgrade::hands_over\n\
        ------------\n\
        \x20    Summary [  90.1s] 401 tests run: 399 passed, 1 failed, 1 timed out\n\
        \x20       FAIL [   1.204s] (  3/401) switchboard::it wait_ci::same_as\n";

    fn set(tests: &[&str]) -> BTreeSet<String> {
        tests.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn the_failing_tests_are_read_from_cargo_s_and_nextest_s_logs() {
        assert_eq!(
            failing_tests(CARGO_LOG),
            Some(set(&["mux::keys::binds_m_p", "notes::id::keeps_its_id"]))
        );
        assert_eq!(
            failing_tests(NEXTEST_LOG),
            Some(set(&["mux::upgrade::hands_over", "wait_ci::same_as"]))
        );
        assert_eq!(
            failing_tests(&format!(
                "{CARGO_LOG}error: test failed, to rerun pass `--lib`\n"
            )),
            Some(set(&["mux::keys::binds_m_p", "notes::id::keeps_its_id"])),
            "cargo's own error for failed tests is the tests'"
        );
        assert_eq!(
            failing_tests(&format!(
                "{CARGO_LOG}error[E0425]: cannot find value `x`\nerror: could not compile `sb`\n"
            )),
            None,
            "a build error beside the tests isn't accounted for by them"
        );
        assert_eq!(failing_tests("all good\n"), Some(BTreeSet::new()));
        assert_eq!(
            failing_tests("test tabs::refuses - should panic ... FAILED\n"),
            Some(set(&["tabs::refuses"])),
            "a should-panic test, named with its mode"
        );
        assert_eq!(
            failing_tests("     TIMEOUT [> 60.000s] switchboard::it tabs::hangs\n"),
            Some(set(&["tabs::hangs"])),
            "a timeout, its duration marked as more than the limit"
        );
        assert_eq!(
            failing_tests("        FAIL [   1.2s] a line of its own\n"),
            None,
            "a nextest failure line it can't read"
        );
        assert_eq!(
            failing_tests(
                "test src/lib.rs - first (line 10) ... FAILED\ntest src/lib.rs - second (line 20) ... FAILED\nerror: doctest failed, to rerun pass `--doc`\n"
            ),
            Some(set(&[
                "src/lib.rs - first (line 10)",
                "src/lib.rs - second (line 20)"
            ])),
            "a doctest keeps its whole name, cargo's error for it the tests'"
        );
        assert_eq!(
            failing_tests("test src/lib.rs - doc (line 3) ... FAILED\n  a ... FAILED\n"),
            None,
            "a failure line it can't read"
        );
        let exited = |how: &str| {
            format!(
                "Caused by:\n  process didn't exit successfully: `target/debug/deps/it-1` ({how})\n"
            )
        };
        assert_eq!(
            failing_tests(&format!(
                "{CARGO_LOG}test result: FAILED. 1 passed; 2 failed\n{}",
                exited("exit status: 101")
            )),
            Some(set(&["mux::keys::binds_m_p", "notes::id::keeps_its_id"])),
            "a binary whose tests failed"
        );
        assert_eq!(
            failing_tests(&format!(
                "{CARGO_LOG}test result: FAILED. 1 passed; 2 failed\n{}",
                exited("signal: 6, SIGABRT: process abort signal")
            )),
            None,
            "a second binary crashing"
        );
        let step_failed = "##[error]Process completed with exit code 101.\n";
        assert_eq!(
            failing_tests(&format!("{CARGO_LOG}{step_failed}")),
            Some(set(&["mux::keys::binds_m_p", "notes::id::keeps_its_id"])),
            "the failed step's own error is the tests'"
        );
        assert_eq!(
            failing_tests(&format!("{CARGO_LOG}{step_failed}##[error]Lint failed\n")),
            None,
            "an Actions error of another kind"
        );
        assert_eq!(
            failing_tests(&format!(
                "{CARGO_LOG}##[error]Process completed with exit code 137.\n"
            )),
            None,
            "a runner killed, for memory"
        );
        assert_eq!(
            failing_tests(&format!("{CARGO_LOG}{step_failed}{step_failed}")),
            None,
            "a second step failing"
        );
        let retried = "  PASS [   0.5s] switchboard::it notes::broken\n\
            \x20 TRY 1 FAIL [   0.5s] switchboard::it tabs::flaky\n\
            \x20 TRY 2 PASS [   0.4s] switchboard::it tabs::flaky\n\
            \x20 TRY 3 FAIL [   0.5s] switchboard::it notes::broken\n\
            \x20 FLAKY 2/3 [   0.4s] switchboard::it tabs::flaky\n";
        assert_eq!(
            failing_tests(retried),
            Some(set(&["notes::broken"])),
            "a test that passed on a retry didn't fail"
        );
    }

    #[test]
    fn a_job_s_partitions_are_one_family_and_its_platforms_are_not() {
        assert_eq!(
            family("Tests (macOS arm64, 1 of 2)"),
            family("Tests (macOS arm64, 2 of 2)")
        );
        assert_eq!(family("Tests (1/2)"), family("Tests (2/2)"));
        assert_ne!(
            family("Tests (Linux x86_64)"),
            family("Tests (macOS arm64, 1 of 2)")
        );
    }

    #[test]
    fn the_passing_tests_are_read_from_cargo_s_and_nextest_s_logs() {
        assert_eq!(passing_tests(CARGO_LOG), set(&["tabs::names_a_tab"]));
        assert_eq!(passing_tests(NEXTEST_LOG), set(&["tabs::names_a_tab"]));
    }

    #[test]
    fn a_landing_s_failure_is_the_base_s_the_change_s_or_mixed_by_its_tests() {
        let base = failing_tests(CARGO_LOG).unwrap();
        let none = BTreeSet::new();
        let same = blame(&parts_of(Some(&base), &base, &none, false));
        assert_eq!(same, Blame::Base, "the same failing tests on both");
        let one = set(&["notes::id::keeps_its_id"]);
        assert_eq!(
            blame(&parts_of(Some(&one), &base, &none, false)),
            Blame::Base,
            "a test of the base's failures alone, whichever job failed it there"
        );
        let new = set(&["sidebar::draws_a_tab"]);
        assert_eq!(
            blame(&parts_of(Some(&new), &base, &new, false)),
            Blame::Change,
            "a test the default branch's run passed"
        );
        assert_eq!(
            blame(&parts_of(Some(&new), &none, &none, true)),
            Blame::Change,
            "a test failing where the default branch's jobs all passed"
        );
        assert_eq!(
            blame(&parts_of(Some(&new), &base, &none, false)),
            Blame::Mixed,
            "a test the default branch's run may never have reached"
        );
        let both = set(&["notes::id::keeps_its_id", "sidebar::draws_a_tab"]);
        assert_eq!(
            blame(&parts_of(Some(&both), &base, &new, false)),
            Blame::Mixed,
            "one of each"
        );
        assert_eq!(
            blame(&parts_of(Some(&none), &base, &none, true)),
            Blame::Mixed,
            "a job that names no failing test, failing to build, can't be told"
        );
        assert_eq!(blame(&parts_of(None, &base, &none, true)), Blame::Mixed);
        assert_eq!(blame(&[Part::Change, Part::Change]), Blame::Change);
        assert_eq!(blame(&[]), Blame::Mixed);
    }

    #[test]
    fn a_commit_s_check_runs_pass_once_all_finished_and_fail_at_the_first_failure() {
        assert_eq!(seen(&[]), Seen::Nothing);
        let done = |name: &str, conclusion: &str| check(name, "completed", json!(conclusion));
        let running = check("Tests", "in_progress", Value::Null);
        assert_eq!(
            seen(&[done("Formatting", "success"), running.clone()]),
            Seen::Running
        );
        assert_eq!(
            seen(&[
                done("Formatting", "success"),
                done("Docs", "skipped"),
                done("Lint", "neutral")
            ]),
            Seen::Passed
        );
        assert_eq!(
            seen(&[done("Tests", "skipped"), done("Lint", "neutral")]),
            Seen::Nothing,
            "jobs that all skipped ran no tests, so the landing runs them itself"
        );
        assert_eq!(
            seen(&[
                done("Formatting", "failure"),
                running,
                done("Build", "cancelled")
            ]),
            Seen::Failed {
                run: "77".into(),
                names: vec!["Formatting".into(), "Build".into()],
                job: Some("5".into()),
            },
            "said while the others still run, a canceled one failing too"
        );
        let status = |state: &str| as_check_run(&json!({ "context": "ci/tests", "state": state }));
        assert_eq!(
            seen(&[done("Formatting", "success"), status("pending")]),
            Seen::Running,
            "a commit status still pending is waited for"
        );
        assert_eq!(
            seen(&[done("Formatting", "success"), status("success")]),
            Seen::Passed
        );
        assert_eq!(
            seen(&[done("Formatting", "success"), status("error")]),
            Seen::Failed {
                run: "-".into(),
                names: vec!["ci/tests".into()],
                job: None,
            }
        );
        let elsewhere = json!({ "name": "other-ci", "status": "completed",
            "conclusion": "timed_out", "details_url": "https://ci.example/build/9" });
        assert_eq!(
            seen(&[elsewhere]),
            Seen::Failed {
                run: "-".into(),
                names: vec!["other-ci".into()],
                job: None,
            },
            "a check run of another CI has no run of GitHub's"
        );
    }
}
