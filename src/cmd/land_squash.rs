//! `land-lock squash`: folds a branch's commits into one, as queue-and-clean's
//! "One commit" and review-loop's "Squashing the loop's commits" do before a
//! push. Those steps had Claude run each check and git command by hand (the
//! clean tree, HEAD still the reviewed commit, a backup branch, `reset
//! --soft`, the commit, the tree compared, the backup restored on a
//! mismatch, `codex-review --record-squash`), where a step skipped or run
//! against the wrong commit loses work or a review; here they run as one
//! command, Claude writing only the message.
//!
//! ```text
//! land-lock squash --message-file <f> [--onto <commit>]
//!     folds the commits after <commit> (by default where HEAD left the
//!     branch it lands on: the target's upstream, else origin's copy of it,
//!     else the target) into one
//!     with <f>'s message; exit 0 squashed, 1 refused or restored with
//!     nothing changed, 3 squashed with the clean review left where it was
//! land-lock squash --drop-backup
//!     deletes the backup branches this worktree's squashes made, once the
//!     push has landed
//! ```
//!
//! It refuses a worktree holding someone else's code (`trust::yours_or_listed`,
//! unless the main clone is in trusted-dirs), whose commit hooks the commit
//! would run, a tree with anything uncommitted, and, when a clean review is
//! recorded for one of the commits it would fold, a HEAD past that commit,
//! since what was committed after it is unreviewed and a squash would hide
//! it. A record for a commit HEAD no longer has (a review before a rebase),
//! or one already below the fold, is left alone. When HEAD is the reviewed commit, the squashed commit takes its
//! place in the record (`codex-review --record-squash`), which checks it
//! makes the same change on the same base.
//!
//! The backup, `backup/pre-squash-<short HEAD>`, is named for the commit,
//! since other worktrees of the repository squash at the same time, and
//! listed in the worktree's own git directory (`land-squash-backups`) for
//! `--drop-backup`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use crate::cmd::codex_review;
use crate::worktree::{git, git_ok};

const USAGE: &str =
    "usage: land-lock squash --message-file <f> [--onto <commit>] | land-lock squash --drop-backup";
/// The backups this worktree's squashes made, in its own git directory.
const BACKUPS: &str = "land-squash-backups";

/// Why a squash stopped, and the exit code it stops with.
struct Stop(u8, String);

/// `land-lock squash`, run in `cwd`; `target` finds the branch it lands
/// on, asked only when no `--onto` names the base.
pub fn main(args: &[String], cwd: &Path, target: impl FnOnce() -> Option<String>) -> ExitCode {
    let result = match args {
        [flag] if flag == "--drop-backup" => drop_backups(cwd),
        _ => match parse(args) {
            Some((message, onto)) => squash(cwd, target, &message, onto.as_deref()),
            None => Err(Stop(2, USAGE.into())),
        },
    };
    match result {
        Ok(said) => {
            println!("{said}");
            ExitCode::SUCCESS
        }
        Err(Stop(code, why)) => {
            eprintln!("land-lock squash: {why}");
            ExitCode::from(code)
        }
    }
}

fn parse(args: &[String]) -> Option<(PathBuf, Option<String>)> {
    let (mut message, mut onto) = (None, None);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--message-file" => message = Some(PathBuf::from(rest.next()?)),
            "--onto" => onto = Some(rest.next()?.clone()),
            _ => return None,
        }
    }
    Some((message?, onto))
}

impl From<anyhow::Error> for Stop {
    fn from(e: anyhow::Error) -> Stop {
        Stop(1, e.to_string())
    }
}

fn squash(
    cwd: &Path,
    target: impl FnOnce() -> Option<String>,
    message: &Path,
    onto: Option<&str>,
) -> Result<String, Stop> {
    let message = cwd.join(message);
    let words = fs::read_to_string(&message)
        .map_err(|e| Stop(1, format!("could not read {}: {e}", message.display())))?;
    if words.trim().is_empty() {
        return Err(Stop(1, format!("{} is empty", message.display())));
    }
    // Checked first, since even git status can run a repository's code (an
    // fsmonitor hook), and the commit runs its commit hooks with your
    // credentials: only where the worktree holds nothing but your own and
    // origin's commits, or the main clone is in trusted-dirs, as land-lock
    // push runs beforePush.
    let here = PathBuf::from(git(cwd, &["rev-parse", "--show-toplevel"])?);
    let home = crate::paths::home();
    let main_clone_listed = crate::cmd::land_lock::common_dir(cwd)
        .and_then(|common| common.parent().map(Path::to_path_buf))
        .is_some_and(|root| crate::trust::listed(&home, &root));
    if crate::trust::is_untrusted(&here)
        || (!main_clone_listed && crate::trust::yours_or_listed(&home, &here).is_err())
    {
        return Err(Stop(
            1,
            "the worktree holds code that isn't yours, so its commit hooks aren't run; trust the main clone in trusted-dirs to squash it".into(),
        ));
    }
    let uncommitted = git(
        cwd,
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    if !uncommitted.is_empty() {
        return Err(Stop(
            1,
            format!("uncommitted changes, nothing squashed:\n{uncommitted}"),
        ));
    }
    let head = git(cwd, &["rev-parse", "HEAD"])?;
    let onto = match onto {
        Some(onto) => git(
            cwd,
            &["rev-parse", "--verify", &format!("{onto}^{{commit}}")],
        )?,
        None => {
            let target = target()
                .ok_or_else(|| Stop(2, "no branch to land on; name the base with --onto".into()))?;
            // Its upstream, else origin's copy (a clone may have no local
            // branch of the default one), else the local branch.
            let exists = |r: &str| git_ok(cwd, &["rev-parse", "--verify", "-q", r]);
            let branch = [
                format!("{target}@{{upstream}}"),
                format!("refs/remotes/origin/{target}"),
            ]
            .into_iter()
            .find(|r| exists(r))
            .unwrap_or(target);
            git(cwd, &["merge-base", "HEAD", &branch])?
        }
    };
    if onto == head || !git_ok(cwd, &["merge-base", "--is-ancestor", &onto, &head]) {
        return Err(Stop(
            1,
            format!("{onto} isn't a commit below HEAD, nothing to squash"),
        ));
    }
    let review = codex_review::recorded_review().map(|(commit, ..)| commit);
    let reviewed_head = review.as_deref() == Some(head.as_str());
    if let Some(reviewed) = review.as_deref().filter(|_| !reviewed_head) {
        let among_the_folded = git_ok(cwd, &["merge-base", "--is-ancestor", reviewed, &head])
            && !git_ok(cwd, &["merge-base", "--is-ancestor", reviewed, &onto]);
        if among_the_folded {
            let since = git(cwd, &["log", "--oneline", &format!("{reviewed}..HEAD")])?;
            return Err(Stop(
                1,
                format!(
                    "HEAD is past the reviewed commit {reviewed}, nothing squashed; committed since:\n{since}"
                ),
            ));
        }
    }

    let short = git(cwd, &["rev-parse", "--short", &head])?;
    let backup = format!("backup/pre-squash-{short}");
    git(cwd, &["branch", "-f", &backup, &head])?;
    let backups = PathBuf::from(git(cwd, &["rev-parse", "--git-path", BACKUPS])?);
    let backups = cwd.join(backups);
    let mut listed = fs::read_to_string(&backups).unwrap_or_default();
    if !listed.lines().any(|l| l == backup) {
        listed.push_str(&format!("{backup}\n"));
        crate::util::write_atomic(&backups, &listed)
            .map_err(|e| Stop(1, format!("could not write {}: {e}", backups.display())))?;
    }
    let tree = git(cwd, &["rev-parse", "HEAD^{tree}"])?;

    git(cwd, &["reset", "-q", "--soft", &onto])?;
    // Run with its output passed through, so whatever a commit hook says
    // about why it refused reaches the caller whole.
    let committed = Command::new("git")
        .args(["commit", "-q", "-F"])
        .arg(&message)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .status();
    if !committed.as_ref().is_ok_and(|s| s.success()) {
        git(
            cwd,
            &["reset", "-q", "--hard", "--no-recurse-submodules", &backup],
        )?;
        let why = match committed {
            Ok(status) => format!("git commit exited {}", status.code().unwrap_or(-1)),
            Err(e) => format!("could not run git commit: {e}"),
        };
        return Err(Stop(
            1,
            format!("the commit failed ({why}, its output above), so HEAD is back at {short}"),
        ));
    }
    let squashed = git(cwd, &["rev-parse", "HEAD"])?;
    if git(cwd, &["rev-parse", "HEAD^{tree}"])? != tree {
        git(
            cwd,
            &["reset", "-q", "--hard", "--no-recurse-submodules", &backup],
        )?;
        return Err(Stop(
            1,
            format!(
                "the squashed commit's tree differs from {short}'s (a commit hook changed it?), so HEAD is back at {backup}"
            ),
        ));
    }
    let short_squashed = git(cwd, &["rev-parse", "--short", &squashed])?;
    let said = format!("squashed into {short_squashed}, backup {backup}");
    if !reviewed_head {
        return Ok(said);
    }
    match codex_review::record_squashed(&squashed) {
        Ok(()) => Ok(format!("{said}; the clean review moved to it")),
        Err(why) => Err(Stop(
            3,
            format!("{said}; the clean review stays put: {why}"),
        )),
    }
}

fn drop_backups(cwd: &Path) -> Result<String, Stop> {
    let backups = cwd.join(git(cwd, &["rev-parse", "--git-path", BACKUPS])?);
    let listed = fs::read_to_string(&backups).unwrap_or_default();
    let (mut dropped, mut kept) = (Vec::new(), Vec::new());
    for branch in listed
        .lines()
        .filter(|l| l.starts_with("backup/pre-squash-"))
    {
        let gone = !git_ok(
            cwd,
            &[
                "rev-parse",
                "--verify",
                "-q",
                &format!("refs/heads/{branch}"),
            ],
        );
        if gone || git_ok(cwd, &["branch", "-q", "-D", branch]) {
            dropped.push(branch);
        } else {
            kept.push(branch);
        }
    }
    // A backup git wouldn't delete (checked out somewhere, say) stays
    // listed, so a later run can drop it.
    if kept.is_empty() {
        let _ = fs::remove_file(&backups);
    } else {
        crate::util::write_atomic(
            &backups,
            kept.iter().map(|b| format!("{b}\n")).collect::<String>(),
        )
        .map_err(|e| Stop(1, format!("could not write {}: {e}", backups.display())))?;
        return Err(Stop(1, format!("could not delete {}", kept.join(", "))));
    }
    Ok(if dropped.is_empty() {
        "no backup to drop".into()
    } else {
        format!("dropped {}", dropped.join(", "))
    })
}
