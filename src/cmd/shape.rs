//! `shape`: starts a Claude session that shapes a Linear ticket with you.
//! Ported from bin/shape.
//!
//! ```text
//! shape <what the ticket is about>
//! shape <linear-ticket-id-or-url> [what to do with it]
//! shape --continue <linear-ticket-id-or-url> [what to do with it]
//! shape --linear | --github | --notes <what the work is about>
//! ```
//!
//! Claude reads the code, checks Linear for tickets that already cover it,
//! drafts the ticket, and creates or updates it once you approve. The
//! instructions live in the /shape skill, so the same thing works inside a
//! session you already have; this only gets the session started in the
//! right place. `--continue` (before the ticket or right after it) carries
//! straight on refining a ticket, without /shape first asking whether to
//! review it. A source (`--linear`, `--github` or `--notes`, before the
//! rest) is passed on to /shape, which files the work there rather than
//! where the repository keeps it; M-p's note actions and shape-batch pass
//! `--linear`, the Linear flows they are named for.
//!
//! It runs in the repository's main checkout, even when called from a
//! worktree, since a ticket being shaped has no branch yet. It fetches first
//! and warns when the local branch is behind, because Claude reads the files
//! on disk. In a Switchboard pane it names the tab "Shaping: <first words>"
//! (its `@tab_name`, as tmux's window option was) so it is easy to find; /wt
//! clears that name when it hands the tab over to the ticket's worktree. The
//! server names its tabs again on its own, so nothing more is asked of it
//! where the tmux version ran window-names.sh.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use crate::mux::socket;
use crate::worktree::{self, fail};

/// How long the fetch that checks the branch is current may take.
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);

fn usage() -> ExitCode {
    eprintln!(
        "Usage: shape [--linear | --github | --notes] <what the work is about>\n       shape [--continue] <linear-ticket-id-or-url> [what to do with it]\n\nExamples:\n  shape \"people can't tell which phase is active\"\n  shape ABC-123\n  shape ABC-123 split out the API part"
    );
    ExitCode::from(1)
}

/// The ticket a word names: an issue URL's, or the word when it is an
/// identifier.
pub fn ticket_in(word: &str) -> Option<String> {
    worktree::ticket_from(word)
}

/// What `shape` was asked: the prompt Claude starts with and the tab's name.
#[derive(Debug, PartialEq, Eq)]
pub struct Asked {
    pub prompt: String,
    pub tab_name: String,
}

/// The sources /shape takes over the repository's.
pub const SOURCES: [&str; 3] = ["--linear", "--github", "--notes"];

/// Reads the arguments (with `--continue` and a source taken out) into the
/// prompt and the tab's name: a ticket alone, or a ticket first and then
/// what to do with it, or an idea in words; `source` goes first in the
/// prompt.
pub fn asked(args: &[String], carry_on: bool, source: Option<&str>) -> Asked {
    let input = args.join(" ").trim().to_string();
    let words: Vec<&str> = input.split_whitespace().collect();
    let first = words.first().copied().unwrap_or("");
    let ticket = ticket_in(first).map(|t| t.to_uppercase());
    let direction = match &ticket {
        Some(_) => input[first.len()..].trim().to_string(),
        None => String::new(),
    };
    let tab_name = match &ticket {
        Some(t) => format!("Shaping: {t}"),
        None => format!(
            "Shaping: {}{}",
            words.iter().take(4).copied().collect::<Vec<_>>().join(" "),
            if words.len() > 4 { "..." } else { "" }
        ),
    };
    let shape = match source {
        Some(s) => format!("/shape {s}"),
        None => "/shape".to_string(),
    };
    let prompt = match ticket {
        Some(t) => format!(
            "{shape} {t}{}{}",
            if carry_on { " --continue" } else { "" },
            if direction.is_empty() {
                String::new()
            } else {
                format!(" {direction}")
            }
        ),
        None => format!("{shape} {input}"),
    };
    Asked { prompt, tab_name }
}

/// The source (only as the first argument), whether to carry on, and the
/// rest. `--continue` counts only before the ticket or right after it:
/// further on it is part of your direction and passes through as written.
pub fn flags(args: &[String]) -> (Option<String>, bool, Vec<String>) {
    let (source, args) = match args.split_first() {
        Some((first, rest)) if SOURCES.contains(&first.as_str()) => (Some(first.clone()), rest),
        _ => (None, args),
    };
    let flag_at = args.iter().take(2).position(|a| a == "--continue");
    let rest = args
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != flag_at)
        .map(|(_, a)| a.clone())
        .collect();
    (source, flag_at.is_some(), rest)
}

pub fn main(args: Vec<String>) -> ExitCode {
    let help = |a: Option<&String>| a.is_some_and(|a| a == "--help" || a == "-h");
    // Help only in place of the ticket or idea: further on, it is direction.
    if help(args.first()) || (args.first().is_some_and(|a| a == "--continue") && help(args.get(1)))
    {
        return usage();
    }
    let (source, carry_on, rest) = flags(&args);
    if rest.is_empty() {
        return usage();
    }
    let asked = asked(&rest, carry_on, source.as_deref());
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(root) = main_checkout(&cwd) else {
        return fail("not inside a git repository");
    };
    warn_if_behind(&root);
    name_tab(&asked.tab_name);
    match Command::new("claude")
        .arg(&asked.prompt)
        .current_dir(&root)
        .status()
    {
        Ok(status) => ExitCode::from(status.code().unwrap_or(0).clamp(0, 255) as u8),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fail("claude not found on PATH"),
        Err(e) => fail(&format!("couldn't start claude: {e}")),
    }
}

/// The main checkout: the parent of the shared `.git` directory, which is
/// the same answer from the main checkout and from any of its worktrees.
fn main_checkout(cwd: &Path) -> Option<PathBuf> {
    super::land_lock::common_dir(cwd)?
        .parent()
        .map(Path::to_path_buf)
}

/// Read-only: no pull, no checkout. The point is only to say when the code
/// Claude is about to read is older than what is on the remote.
fn warn_if_behind(root: &Path) {
    let branch = worktree::git_at(root, &["branch", "--show-current"]).unwrap_or_default();
    let upstream =
        worktree::git_at(root, &["rev-parse", "--abbrev-ref", "@{upstream}"]).unwrap_or_default();
    if branch.is_empty() || upstream.is_empty() {
        return;
    }
    if !fetch(root) {
        eprintln!("Could not fetch; shaping against the local checkout as it is.");
        return;
    }
    let behind = worktree::git_at(
        root,
        &["rev-list", "--count", &format!("{branch}..{upstream}")],
    )
    .ok()
    .and_then(|n| n.parse::<u64>().ok())
    .unwrap_or(0);
    if behind > 0 {
        eprintln!(
            "Note: {branch} is {behind} commit{} behind {upstream}. Claude reads the files on disk, so update it first if those commits matter here.",
            if behind == 1 { "" } else { "s" }
        );
    }
}

/// `git fetch --quiet` in the checkout, given up on after `FETCH_TIMEOUT`.
fn fetch(root: &Path) -> bool {
    let Ok(mut child) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["fetch", "--quiet"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = Instant::now() + FETCH_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Names the tab, as M-p's "Rename the tab" does, when this runs in a
/// Switchboard pane. A tab without a name is no reason to stop.
fn name_tab(label: &str) {
    if socket::in_pane() {
        let _ = socket::ask(&["set", "@tab_name", label]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_ticket_is_shaped_by_its_identifier_with_the_direction_after_it() {
        assert_eq!(
            asked(&args(&["abc-12", "split", "out", "the", "API"]), true, None),
            Asked {
                prompt: "/shape ABC-12 --continue split out the API".into(),
                tab_name: "Shaping: ABC-12".into(),
            }
        );
        assert_eq!(
            asked(
                &args(&["https://linear.app/w/issue/ENG-5/title"]),
                false,
                None
            )
            .prompt,
            "/shape ENG-5"
        );
    }

    #[test]
    fn an_idea_is_passed_as_written_and_the_tab_named_by_its_first_words() {
        let idea = asked(
            &args(&["people can't tell which phase is active"]),
            true,
            None,
        );
        assert_eq!(
            idea.prompt,
            "/shape people can't tell which phase is active"
        );
        assert_eq!(idea.tab_name, "Shaping: people can't tell which...");
        assert_eq!(
            asked(&args(&["slow results"]), false, None).tab_name,
            "Shaping: slow results"
        );
    }

    #[test]
    fn a_source_goes_first_in_the_prompt_and_stays_out_of_the_tabs_name() {
        let idea = asked(&args(&["slow results"]), false, Some("--linear"));
        assert_eq!(idea.prompt, "/shape --linear slow results");
        assert_eq!(idea.tab_name, "Shaping: slow results");
        assert_eq!(
            asked(&args(&["ABC-7"]), true, Some("--linear")).prompt,
            "/shape --linear ABC-7 --continue"
        );
    }

    #[test]
    fn a_source_counts_only_first_and_continue_only_before_or_after_the_ticket() {
        let f = |list: &[&str]| flags(&args(list));
        assert_eq!(
            f(&["--linear", "--continue", "ABC-7"]),
            (Some("--linear".into()), true, args(&["ABC-7"]))
        );
        assert_eq!(
            f(&["ABC-123", "add", "--continue", "support"]),
            (
                None,
                false,
                args(&["ABC-123", "add", "--continue", "support"])
            ),
            "further on, it is direction"
        );
        assert_eq!(
            f(&["ABC-1", "--github"]),
            (None, false, args(&["ABC-1", "--github"]))
        );
    }
}
