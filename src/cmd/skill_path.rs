//! `skill-path <name> [<dir>]`: the SKILL.md to follow for the skill `<name>`
//! in the repository at `<dir>` (the current directory when none is given).
//! Ported from `bin/skill-path`. `skill-path --own <name>` prints the text of
//! Switchboard's own copy, which the binary carries, where the Node version's
//! skills read it from `~/switchboard/claude/skills`.
//!
//! The worktree's own `.claude/skills/<name>/SKILL.md` comes first, then its
//! main clone's (a branch cut before the repository added the skill has none
//! of its own), then Switchboard's own: the personal one,
//! `~/.claude/skills/<name>/SKILL.md`, when it is Switchboard's (written
//! there by an earlier version, or a copy of it made yours,
//! `skills::switchboard_s`), else the plugin's,
//! `~/.local/share/switchboard/claude/plugin/skills/<name>/SKILL.md`
//! (src/setup/plugin.rs). Outside a git repository, Switchboard's own alone.
//! It prints nothing and exits 1 when there is none.
//!
//! Every Switchboard skill runs it first and follows the repository's copy
//! when it prints one, and a skill handing off to another reads the file it
//! prints, since Claude Code itself may run Switchboard's copy where a
//! project has its own: a personal skill over a project's of the same name,
//! and the plugin's whenever it is asked for by its plugin's name
//! (`/switchboard:<name>`) or a hand-off names it. A skill of yours that only
//! shares a name with one of Switchboard's is never printed: Claude Code
//! runs it for the bare name, and Switchboard's, run as
//! `/switchboard:<name>` or handed off to, follows its own text rather than
//! yours.
//!
//! In a worktree of someone else's PR (`wtpr --untrusted`, src/trust.rs),
//! the worktree's own copy is passed over, saying so on stderr, and its
//! main clone's comes first: its skills are the PR author's, and a
//! review-loop of theirs would rewrite the review of their change.
//!
//! A build from a clone reads its skills from the clone (src/assets.rs), so
//! a skill can name a command the installed binary, built before it, lacks:
//! a landing once ran `land-lock test` on such a binary, read its usage line
//! as no CI answering, and ran the tests here. So beside the path, on
//! stderr, it says which `switchboard` commands the skill's `allowed-tools`
//! name that this build doesn't have (`missing_commands`, against
//! `cmd::has`), and the skills that act on it stop there.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::{paths, worktree};

pub fn main(args: Vec<String>) -> ExitCode {
    if args.first().map(String::as_str) == Some("--own") {
        return own(args.get(1).map(String::as_str));
    }
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
            let missing = std::fs::read_to_string(&found)
                .map(|text| missing_commands(&text))
                .unwrap_or_default();
            if !missing.is_empty() {
                eprintln!("{}", behind_line(&missing));
            }
            ExitCode::SUCCESS
        }
        None => ExitCode::from(1),
    }
}

/// `--own <name>`: Switchboard's own SKILL.md, as this build reads it
/// (the clone's, else what the binary carries: src/assets.rs), for
/// a hand-off that needs Switchboard's text whatever copy is installed
/// (queue-and-clean reading ci-loop's "When queue-and-clean runs this"),
/// since `~/.claude/skills/<name>` may be a skill of your own by that name.
fn own(name: Option<&str>) -> ExitCode {
    let Some(name) = name.filter(|n| !n.is_empty() && !n.starts_with('-')) else {
        eprintln!("usage: skill-path --own <name>");
        return ExitCode::from(2);
    };
    match crate::setup::skills::shipped_file(&format!("{name}/SKILL.md")) {
        Some(text) => {
            print!("{}", String::from_utf8_lossy(&text));
            ExitCode::SUCCESS
        }
        None => ExitCode::from(1),
    }
}

/// The line saying this build is older than the skill, naming what it lacks.
pub fn behind_line(missing: &[String]) -> String {
    format!(
        "skill-path: build behind: this switchboard lacks {}, which the skill names: build and install Switchboard from main, then run the skill again",
        missing
            .iter()
            .map(|m| format!("`switchboard {m}`"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// The `switchboard` commands a SKILL.md's `allowed-tools` name that this
/// build lacks, each as `<program>` or `<program> <subcommand>`, once each.
pub fn missing_commands(skill: &str) -> Vec<String> {
    let mut lines = skill.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return Vec::new();
    }
    let mut missing: Vec<String> = Vec::new();
    for line in lines.take_while(|l| l.trim_end() != "---") {
        // A YAML list item, its rule bare or quoted either way.
        let item = line.trim().trim_start_matches("- ").trim();
        let unquoted = ['"', '\'']
            .into_iter()
            .find_map(|q| item.strip_prefix(q).and_then(|i| i.strip_suffix(q)))
            .unwrap_or(item);
        let Some(rule) = unquoted
            .strip_prefix("Bash(switchboard ")
            .and_then(|r| r.strip_suffix(')'))
        else {
            continue;
        };
        let mut words = rule.split_whitespace();
        let Some(name) = words.next().filter(|n| !n.contains('*')) else {
            continue;
        };
        let first = words.next();
        if crate::cmd::has(name, first) {
            continue;
        }
        let what = match first {
            Some(first) if crate::cmd::is_command(name) => format!("{name} {first}"),
            _ => name.to_string(),
        };
        if !missing.contains(&what) {
            missing.push(what);
        }
    }
    missing
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
        // A worktree of someone else's PR, checked out untrusted: its own
        // skills are its author's, and a review-loop of theirs would rewrite
        // the review of their change (src/trust.rs).
        if crate::trust::is_untrusted(&top) {
            eprintln!(
                "skill-path: {} holds an untrusted PR, so its own skills are passed over",
                top.display()
            );
        } else {
            roots.push(top.clone());
        }
        if let Some(main) = worktree::repo_root_of(&top) {
            roots.push(main);
        }
    }
    if crate::setup::skills::switchboard_s(name) {
        roots.push(paths::home());
    }
    roots
        .into_iter()
        .map(|root| root.join(".claude/skills").join(name).join("SKILL.md"))
        .chain(std::iter::once(
            paths::plugin_dir()
                .join("skills")
                .join(name)
                .join("SKILL.md"),
        ))
        .find(|file| file.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_what_this_build_lacks_once_each() {
        let skill = "---\nname: x\nallowed-tools:\n  - Bash(switchboard land-lock take)\n  - Bash(switchboard land-lock nosuch)\n  - Bash(switchboard land-lock nosuch --here *)\n  - Bash(switchboard no-such-program *)\n  - \"Bash(switchboard land-lock quoted)\"\n  - 'Bash(switchboard land-lock take)'\n  - Bash(git status)\n---\n\nBash(switchboard after-the-front-matter)\n";
        assert_eq!(
            missing_commands(skill),
            vec![
                "land-lock nosuch".to_string(),
                "no-such-program".to_string(),
                "land-lock quoted".to_string()
            ]
        );
    }

    #[test]
    fn a_skill_with_no_front_matter_names_nothing() {
        assert!(missing_commands("Bash(switchboard nosuch)").is_empty());
    }
}
