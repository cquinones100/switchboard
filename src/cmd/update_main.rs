//! `update-main [<dir>]`: brings the repository's local default branch (main,
//! as `origin/HEAD` names it) up to date with origin's, from wherever you are:
//! a worktree on another branch, or the main clone itself. Ported from
//! `bin/update-main`.
//!
//! Nothing is switched, and only a fast forward is ever made, so local
//! commits on main are never lost. Where main is checked out (usually the
//! main clone), it is fast-forwarded there, which moves that checkout's files
//! too, so a checkout with changes not committed is left alone and said so.
//! Where it is checked out nowhere, the branch is moved without touching any
//! files. Either way, a main with commits origin's doesn't have is left as it
//! is.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use crate::worktree::{self, git_at};

fn fail(message: &str) -> ExitCode {
    eprintln!("update-main: {message}");
    ExitCode::from(1)
}

pub fn main(args: Vec<String>) -> ExitCode {
    if args.iter().any(|a| a == "--help" || a == "-h") || args.len() > 1 {
        eprintln!("Usage: update-main [<dir>]");
        return ExitCode::from(if args.len() > 1 { 1 } else { 0 });
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let dir = worktree::normalize(&cwd.join(args.first().map(String::as_str).unwrap_or(".")));
    match update(&dir) {
        Ok(said) => {
            println!("{said}");
            ExitCode::SUCCESS
        }
        Err(why) => fail(&why),
    }
}

/// Does the update; what it did, or why it didn't.
fn update(dir: &Path) -> Result<String, String> {
    // The main clone, from the top of the checkout `dir` is in: older git
    // gives `--git-common-dir` relative to the top whatever directory asks,
    // and knows no `--path-format`.
    let outside = || format!("not in a git repository: {}", dir.display());
    let top = git_at(dir, &["rev-parse", "--show-toplevel"]).map_err(|_| outside())?;
    let root = worktree::repo_root_of(Path::new(&top)).ok_or_else(outside)?;
    let remote = worktree::default_remote(&root).ok_or("the repository has no remote")?;
    let branch = git_at(
        &root,
        &["rev-parse", "--abbrev-ref", &format!("{remote}/HEAD")],
    )
    .unwrap_or_else(|_| format!("{remote}/main"))
    .replacen(&format!("{remote}/"), "", 1);

    let fetched = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args([
            "fetch",
            "--quiet",
            &remote,
            &format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .is_ok_and(|s| s.success());
    if !fetched {
        return Err(format!("could not fetch {branch} from {remote}"));
    }
    let git = |args: &[&str]| git_at(&root, args).map_err(|e| e.to_string());
    let theirs = git(&["rev-parse", &format!("{remote}/{branch}")])?;
    let ours = git_at(&root, &["rev-parse", &format!("refs/heads/{branch}")]).ok();
    let short = |sha: &str| sha.chars().take(9).collect::<String>();
    let count = |from: &str, to: &str| git(&["rev-list", "--count", &format!("{from}..{to}")]);

    if ours.as_deref() == Some(theirs.as_str()) {
        return Ok(format!(
            "{branch} is up to date with {remote} ({}).",
            short(&theirs)
        ));
    }
    if let Some(ours) = &ours
        && !worktree::git_ok(&root, &["merge-base", "--is-ancestor", ours, &theirs])
    {
        return Err(format!(
            "{branch} has {} commit(s) {remote}/{branch} doesn't, so it was left as it is.",
            count(&theirs, ours)?
        ));
    }

    // The worktree main is checked out in, if any.
    let checked_out = worktree::holding_branch(&root, &branch);
    let behind = match &ours {
        Some(ours) => count(ours, &theirs)?,
        None => "all".to_string(),
    };
    match &checked_out {
        Some(at) => {
            let status = git_at(at, &["status", "--porcelain", "--untracked-files=no"])
                .map_err(|e| e.to_string())?;
            if !status.is_empty() {
                return Err(format!(
                    "{branch} is checked out in {}, which has changes not committed, so it was left at {}, {behind} commit(s) behind.",
                    at.display(),
                    short(ours.as_deref().unwrap_or(""))
                ));
            }
            git_at(
                at,
                &[
                    "merge",
                    "--ff-only",
                    "--quiet",
                    &format!("{remote}/{branch}"),
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        None => {
            let reference = format!("refs/heads/{branch}");
            let mut args = vec!["update-ref", reference.as_str(), theirs.as_str()];
            if let Some(ours) = &ours {
                args.push(ours);
            }
            git(&args)?;
        }
    }
    Ok(format!(
        "{branch}: {} → {}, {behind} commit(s) from {remote}{}.",
        ours.as_deref()
            .map(short)
            .unwrap_or_else(|| "(none)".into()),
        short(&theirs),
        checked_out
            .map(|at| format!(", in {}", at.display()))
            .unwrap_or_default()
    ))
}
