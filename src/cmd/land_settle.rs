//! `land-lock settle [--command <cmd>] [--patch <id>]`: the rebase, retest
//! and compare that come between a landing's push question and its push,
//! queue-and-clean's steps 8 to 10 in one command, where the skill had
//! Claude take or wait for the lock, fetch, rebase, choose between
//! `afterRebase` and the tests, set each phase, compare patch-ids and
//! requeue, each a step a session could get wrong. New with the Rust
//! version.
//!
//! In order:
//!
//! 1. It takes the landing lock, or waits in the queue for its turn, as
//!    `take` and `wait` do (a landing back from a `requeue` waiting
//!    straight away); taken out of the queue meanwhile, it exits 4. It goes
//!    on only where a landing may run the repository's code
//!    (`land_push::may_run_its_code`, since git's status and rebase run its
//!    hooks and file-system monitor), else exiting 11 `untrusted`.
//! 2. It fetches the branch landed on (none when it tracks no remote), and
//!    rebases onto it unless it is already under the commit (phase
//!    `rebasing again`). A conflict is left in the worktree, the lock let
//!    go and the landing's record marked (`settle.conflict`), exit 10: a
//!    change resolved by hand is tested in full and reviewed again on the
//!    next run, whatever its patch.
//! 3. It tests what it has to. A commit the landing already tested or
//!    checked (the record's `tests.head`, which `land-lock test` writes,
//!    or `settle.checked`) is left as it is. After a clean rebase with
//!    repos.json's `afterRebase` set, while the branch landed on isn't
//!    failing its own CI at one look (`wait_ci::failing_now`, an answer
//!    needed), the tests passed on the change and on that branch, so only
//!    `afterRebase` runs, checking what doesn't build or lint together
//!    (phase `checking after the rebase`, `check failed` when it fails,
//!    its output in `landing-check.log` in the worktree's own git
//!    directory). Otherwise the tests run again, as `land-lock test --again
//!    --base <branch>` runs them (with `--command` when given), here or in
//!    CI. A failure lets the lock go and exits 11, the tests' own last
//!    line, or `check failed <log>`, last.
//! 4. It compares the change with the one reviewed: the patch
//!    `codex-review --recorded` names (or, for a landing with no review,
//!    `--patch`, the one asked about) against `codex-review --patch-id`
//!    from the branch to HEAD. Different, or a conflict since, it runs
//!    `requeue` (the lock let go, this worktree first in line, marked
//!    reviewing) and exits 12, for the review or the question again.
//!    The same, it exits 0 holding the lock, printing `ready <short hash>`,
//!    for `land-lock push`.
//!
//! `--command` comes first or not at all, as `land-lock test`'s does, so a
//! rule allowing `land-lock settle --patch *` never lets a command of
//! Claude's choosing through. Every way out but 0 and 12 lets the lock go
//! and takes this worktree out of the queue.

use std::fs;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use serde_json::{Value, json};

use super::land_lock::{Lock, set_phase};
use super::land_push::{self, git, this_program};
use crate::sources::landing::{RECORD, own_git_dir, read_record};
use crate::{repos, util, worktree};

const USAGE: &str = "usage: land-lock settle [--command <cmd>] [--patch <id>]";
/// Taken out of the landing queue while waiting.
const OUT_OF_QUEUE: u8 = 4;
const CONFLICT: u8 = 10;
const FAILED: u8 = 11;
const CHANGED: u8 = 12;
/// `afterRebase`'s output, in the worktree's own git directory.
const CHECK_LOG: &str = "landing-check.log";
/// How many of a failing check's last lines it prints.
const SHOWN: usize = 40;

pub(crate) fn main(
    lock: &Lock,
    common: &Path,
    here: &Path,
    target: Option<String>,
    args: &[String],
) -> ExitCode {
    let Some((command, patch)) = parse(args) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match lock.take_or_wait() {
        Ok(0) => {}
        Ok(_) => {
            println!("taken out of the landing queue");
            return ExitCode::from(OUT_OF_QUEUE);
        }
        Err(e) => {
            eprintln!("land-lock settle: {e}");
            return ExitCode::FAILURE;
        }
    }
    let code = settle(lock, common, here, target, command, patch);
    if code != 0 && code != CHANGED {
        let _ = lock.release();
    }
    ExitCode::from(code)
}

/// `--command`'s and `--patch`'s values; `None` for anything else, or
/// `--command` after another argument.
fn parse(args: &[String]) -> Option<(Option<String>, Option<String>)> {
    let mut rest = args;
    let mut command = None;
    if rest.first().map(String::as_str) == Some("--command") {
        command = Some(rest.get(1)?.clone());
        rest = &rest[2..];
    }
    match rest {
        [] => Some((command, None)),
        [flag, id] if flag == "--patch" && !id.starts_with("--") => {
            Some((command, Some(id.clone())))
        }
        _ => None,
    }
}

fn settle(
    lock: &Lock,
    common: &Path,
    here: &Path,
    target: Option<String>,
    command: Option<String>,
    patch: Option<String>,
) -> u8 {
    // git runs the repository's hooks and file-system monitor (a status, a
    // rebase), as the tests and the check run its code: never in a worktree
    // of someone else's, as `land-lock squash` refuses it.
    if !land_push::may_run_its_code(common, here) {
        println!(
            "the worktree holds code that isn't yours, so it isn't rebased or tested here; trust the main clone in trusted-dirs to land it"
        );
        println!("untrusted");
        return FAILED;
    }
    let Some(own) = own_git_dir(here) else {
        eprintln!("land-lock settle: not in a worktree");
        return 1;
    };
    if own.join("rebase-merge").exists() || own.join("rebase-apply").exists() {
        println!("a rebase is still going on here: resolve it and continue it, then settle again");
        println!("conflict");
        return CONFLICT;
    }
    if !worktree::git_at(here, &["status", "--porcelain"]).is_ok_and(|s| s.is_empty()) {
        eprintln!(
            "land-lock settle: the worktree has uncommitted changes: commit or drop them first"
        );
        return 1;
    }
    let steps = match repos::steps_at(here) {
        Ok(steps) => steps,
        Err(e) => {
            eprintln!("land-lock settle: {e}");
            return 1;
        }
    };
    // `land-lock test` is given the same target, so it reads the branch
    // and its remote as the push does.
    let onto = match land_push::onto(common, here, target.clone()) {
        Ok(onto) => onto,
        Err(e) => {
            eprintln!("land-lock settle: {e}");
            return 1;
        }
    };
    if let Err(e) = land_push::fetch(here, &onto) {
        eprintln!("land-lock settle: {e}");
        return 1;
    }
    let base = onto.base();
    let record = read_record(here).unwrap_or_else(|| json!({}));
    let had_conflict = record.pointer("/settle/conflict") == Some(&Value::Bool(true));
    let head = || worktree::git_at(here, &["rev-parse", "HEAD"]).unwrap_or_default();

    let moved = !worktree::git_ok(here, &["merge-base", "--is-ancestor", &base, "HEAD"]);
    if moved {
        set_phase("rebasing again");
        let (rebased, said) = git(here, &["rebase", &base]);
        if !rebased {
            if !(own.join("rebase-merge").exists() || own.join("rebase-apply").exists()) {
                eprintln!("land-lock settle: the rebase onto {base} failed:\n{said}");
                return 1;
            }
            mark(&own, json!({ "conflict": true }));
            set_phase("rebase conflict");
            if let Ok(files) = worktree::git_at(here, &["diff", "--name-only", "--diff-filter=U"]) {
                for file in files.lines() {
                    println!("conflict in {file}");
                }
            }
            println!("conflict");
            return CONFLICT;
        }
    }

    let now = head();
    let tested = |key: &str, ok_needed: bool| {
        record.get(key).is_some_and(|run| {
            run.get("head").and_then(Value::as_str) == Some(now.as_str())
                && (!ok_needed || run.get("ok") == Some(&Value::Bool(true)))
        })
    };
    let already = tested("tests", true) || tested("settle", false);
    if !already {
        let check_only = moved
            && !had_conflict
            && !steps.after_rebase.trim().is_empty()
            && base_ci_passing(here, &onto, &base);
        let passed = if check_only {
            check(common, here, &own, &steps.after_rebase)
        } else {
            retest(&base, command, target)
        };
        if !passed {
            return FAILED;
        }
        mark(&own, json!({ "conflict": had_conflict, "head": now }));
    }

    let current = this_program(here, &["codex-review", "--patch-id", &base, "HEAD"])
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let reviewed = if steps.review == "review-loop" {
        this_program(here, &["codex-review", "--recorded"])
            .filter(|o| o.status.success())
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split_whitespace()
                    .nth(2)
                    .map(str::to_string)
            })
    } else {
        patch
    };
    let same = matches!((&reviewed, &current), (Some(r), Some(c)) if !r.is_empty() && r == c);
    if had_conflict || !same {
        mark(&own, json!({ "conflict": false, "head": now }));
        if let Err(e) = lock.requeue() {
            eprintln!("land-lock settle: {e}");
            return 1;
        }
        let why = if had_conflict {
            "the rebase had a conflict"
        } else if steps.review == "review-loop" {
            "the change isn't the one the review found clean"
        } else {
            "the change isn't the one asked about"
        };
        println!("{why}: the lock is let go, this worktree first in line to land");
        println!("changed");
        return CHANGED;
    }
    let short = worktree::git_at(here, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    println!("ready {short}");
    0
}

/// Keeps `settle` in the landing's record, which `land-lock record` can't
/// write; a record failing never stops the landing.
fn mark(own: &Path, settle: Value) {
    let path = own.join(RECORD);
    let mut record = util::read_json_object(&path);
    record.insert("settle".into(), settle);
    let _ = util::write_atomic(&path, util::json_compact(&Value::Object(record)));
}

/// Whether the branch landed on answers as not failing its own CI at one
/// look; a branch on no GitHub remote, or no answer, says nothing.
fn base_ci_passing(here: &Path, onto: &land_push::Onto, base: &str) -> bool {
    let Some((remote, _)) = &onto.remote else {
        return false;
    };
    let Some(repo) = worktree::git_at(here, &["config", "--get", &format!("remote.{remote}.url")])
        .ok()
        .and_then(|url| repos::repo_of_url(&url))
    else {
        return false;
    };
    let Ok(sha) = worktree::git_at(here, &["rev-parse", base]) else {
        return false;
    };
    crate::integrations::run_by_you();
    super::wait_ci::failing_now(&repo, &sha) == Some(false)
}

/// Runs `afterRebase` in the worktree, as the module's comment says.
fn check(common: &Path, here: &Path, own: &Path, after_rebase: &str) -> bool {
    if !land_push::may_run_its_code(common, here) {
        println!(
            "the worktree holds code that isn't yours, so afterRebase doesn't run; trust the main clone in trusted-dirs to land it"
        );
        println!("untrusted");
        return false;
    }
    set_phase("checking after the rebase");
    let log = own.join(CHECK_LOG);
    let ran = fs::File::create(&log).and_then(|out| {
        Command::new("sh")
            .args(["-c", after_rebase])
            .current_dir(here)
            .stdin(Stdio::null())
            .stdout(out.try_clone()?)
            .stderr(out)
            .status()
    });
    match ran {
        Ok(status) if status.success() => true,
        Ok(_) => {
            let text = String::from_utf8_lossy(&fs::read(&log).unwrap_or_default()).into_owned();
            let lines: Vec<&str> = text.lines().collect();
            for line in &lines[lines.len().saturating_sub(SHOWN)..] {
                println!("{line}");
            }
            set_phase("check failed");
            println!("check failed {}", log.display());
            false
        }
        Err(e) => {
            println!("couldn't run afterRebase ({after_rebase}): {e}");
            set_phase("check failed");
            println!("check failed {}", log.display());
            false
        }
    }
}

/// The tests again, as `land-lock test --again --base <base>` runs them.
fn retest(base: &str, command: Option<String>, target: Option<String>) -> bool {
    let mut args = Vec::new();
    if let Some(command) = command {
        args.extend(["--command".to_string(), command]);
    }
    args.extend([
        "--again".to_string(),
        "--base".to_string(),
        base.to_string(),
    ]);
    super::land_test::main(&args, target) == ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn a_command_is_read_only_before_the_patch() {
        assert_eq!(parse(&[]), Some((None, None)));
        assert_eq!(
            parse(&words(&["--patch", "abc"])),
            Some((None, Some("abc".into())))
        );
        assert_eq!(
            parse(&words(&["--command", "make test", "--patch", "abc"])),
            Some((Some("make test".into()), Some("abc".into())))
        );
        assert_eq!(parse(&words(&["--patch", "abc", "--command", "x"])), None);
        assert_eq!(parse(&words(&["--patch"])), None);
        assert_eq!(parse(&words(&["--again"])), None);
    }
}
