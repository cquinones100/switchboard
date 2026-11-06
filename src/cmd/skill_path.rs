//! `skill-path <name> [<dir>]`: the SKILL.md to follow for the skill `<name>`
//! in the repository at `<dir>` (the current directory when none is given).
//! Ported from `bin/skill-path`.
//!
//! The worktree's own `.claude/skills/<name>/SKILL.md` comes first, then its
//! main clone's (a branch cut before the repository added the skill has none
//! of its own), then the personal one, `~/.claude/skills/<name>/SKILL.md`.
//! Outside a git repository, the personal one. It prints nothing and exits 1
//! when there is none.
//!
//! Every Switchboard skill runs it first and follows the repository's copy
//! when it prints one, and a skill handing off to another reads the file it
//! prints, since Claude Code itself runs a personal skill over a project's
//! of the same name.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::{paths, worktree};

pub fn main(args: Vec<String>) -> ExitCode {
    let Some(name) = args
        .first()
        .filter(|n| !n.is_empty() && !n.starts_with('-'))
    else {
        eprintln!("usage: skill-path <name> [<dir>]");
        return ExitCode::from(2);
    };
    let dir = args
        .get(1)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    match find(name, &dir) {
        Some(found) => {
            println!("{}", found.display());
            ExitCode::SUCCESS
        }
        None => ExitCode::from(1),
    }
}

/// The SKILL.md to follow, as the module's comment orders them.
pub fn find(name: &str, dir: &Path) -> Option<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(top) = worktree::git_at(dir, &["rev-parse", "--show-toplevel"])
        && !top.is_empty()
    {
        // Asked from the worktree's top, where `--git-common-dir` is
        // relative to (or absolute), on every git version: older ones gave
        // it relative to the top whatever directory asked, and don't know
        // `--path-format`.
        let top = PathBuf::from(top);
        if let Some(main) = worktree::repo_root_of(&top) {
            roots.push(top);
            roots.push(main);
        } else {
            roots.push(top);
        }
    }
    roots.push(paths::home());
    roots
        .into_iter()
        .map(|root| root.join(".claude/skills").join(name).join("SKILL.md"))
        .find(|file| file.exists())
}
