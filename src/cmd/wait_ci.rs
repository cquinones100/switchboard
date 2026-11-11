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
//! ```
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

use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::wait_deploy::refused;
use crate::integrations::github;
use crate::util;

const POLL: Duration = Duration::from_secs(30);
const SOURCE: &str = "wait-ci";
const USAGE: &str =
    "usage: wait-ci [--now] --repo <owner/name> --sha <commit> [--timeout 45] [--grace 3]";

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
    Ok((sha, seen(&all)))
}

fn seen(check_runs: &[Value]) -> Seen {
    let failed: Vec<&Value> = check_runs
        .iter()
        .filter(|c| {
            text(c, "status") == "completed"
                && !matches!(text(c, "conclusion"), "success" | "skipped" | "neutral")
        })
        .collect();
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
    let failed = |run: &str, names: &[String]| format!("failed {sha} {run} {}", names.join(", "));
    if args.iter().any(|a| a == "--now") {
        return match util::block_on(look(&repo, &sha, SOURCE)) {
            Ok((_, now)) => {
                println!(
                    "{}",
                    match now {
                        Seen::Failed { run, names, .. } => failed(&run, &names),
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
        let looked = util::block_on(look(&repo, &sha, SOURCE));
        let now = match looked {
            Ok((_, seen)) => Some(seen),
            // No token, or one GitHub refuses, won't mend by waiting, so
            // it is said at once rather than spending the whole timeout.
            Err(e) if refused(&e) => {
                eprintln!("wait-ci: {e}");
                return ExitCode::FAILURE;
            }
            // A network blip or a rate limit: try again next round.
            Err(_) => None,
        };
        let answer = match &now {
            Some(Seen::Failed { run, names, .. }) => Some(failed(run, names)),
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
