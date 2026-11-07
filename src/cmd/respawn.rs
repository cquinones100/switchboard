//! `switchboard respawn [-t pane] [-c dir] <command> [args...]`: the pane's
//! program replaced by another, in the same place, as the skills used
//! `tmux respawn-pane -k` to hand a tab over: /wt and /wtn to the worktree
//! command that opens the work, and queue-and-clean to `claude --resume` in
//! the main clone once the worktree it ran in is gone.
//!
//! `wt`, `wtn`, `wtb` and `wtpr` run as the worktree commands' shell
//! functions, defined from their `init` in a shell of yours
//! (`worktree::tab_running`), so they work whether or not your rc defines
//! them; anything else runs as it is, in an interactive shell that reads
//! your rc for the PATH. Either way a shell is left in the pane when it
//! ends, in whatever directory it left: the worktree's, after `wt`. The
//! arguments are words of their own, never read as shell, so a prompt or a
//! note's path can hold anything, where the tmux version passed them in the
//! environment to keep them out of the command line.
//!
//! The server has no respawn of its own: the new program starts in a pane
//! split from this one, left in the background, and this one is closed,
//! which gives the new pane its place, so the tab, its options and its name
//! stay as they were. When the pane was the one in front in its tab, the new
//! one is made so. Closing the pane ends the Claude session that asked for
//! this, so it is the last thing that session runs.

use std::process::ExitCode;

use serde_json::Value;

use crate::mux::socket;
use crate::worktree;

/// The worktree commands' shell functions, and the programs that define
/// them.
const FUNCTIONS: &[(&str, &str)] = &[
    ("wt", "worktree-from-ticket"),
    ("wtn", "worktree-from-note"),
    ("wtb", "worktree-from-branch"),
    ("wtpr", "worktree-from-pr"),
];

fn usage() -> ExitCode {
    eprintln!(
        "Usage: switchboard respawn [-t pane] [-c dir] <command> [args...]\n\n\
         Replaces the pane's program (this pane's, without -t) with <command>,\n\
         started in <dir>. wt, wtn, wtb and wtpr run as the worktree commands'\n\
         shell functions. A shell is left in the pane when it ends."
    );
    ExitCode::from(2)
}

/// The command the new pane runs.
pub fn command_for(program: &str, args: &[String]) -> Vec<String> {
    match FUNCTIONS.iter().find(|(f, _)| *f == program) {
        Some((function, defined_by)) => worktree::tab_running(defined_by, function, args),
        None => {
            let mut words = vec![program.to_string()];
            words.extend(args.iter().cloned());
            worktree::tab_command("\"$@\"", program, &words)
        }
    }
}

/// `switchboard respawn`.
pub fn main(args: Vec<String>) -> ExitCode {
    let mut pane = std::env::var("SWITCHBOARD_PANE")
        .ok()
        .filter(|p| !p.is_empty());
    let mut dir: Option<String> = None;
    let mut rest = args.as_slice();
    loop {
        match rest {
            [flag, value, more @ ..] if flag == "-t" => {
                pane = Some(value.clone());
                rest = more;
            }
            [flag, value, more @ ..] if flag == "-c" => {
                dir = Some(value.clone());
                rest = more;
            }
            [dashes, more @ ..] if dashes == "--" => {
                rest = more;
                break;
            }
            _ => break,
        }
    }
    let Some((program, program_args)) = rest.split_first() else {
        return usage();
    };
    if program.starts_with('-') {
        return usage();
    }
    let Some(pane) = pane else {
        eprintln!("switchboard respawn: not in a Switchboard pane, and no -t given");
        return ExitCode::from(1);
    };
    match respawn(&pane, dir.as_deref(), &command_for(program, program_args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("switchboard respawn: {e:#}");
            ExitCode::from(1)
        }
    }
}

/// Whether the pane is the one in front in its tab, from `list`.
fn in_front(list: &Value, pane: &str) -> bool {
    list["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| s["tabs"].as_array().into_iter().flatten())
        .flat_map(|t| t["panes"].as_array().into_iter().flatten())
        .any(|p| p["id"] == pane && p["active"] == true)
}

fn respawn(pane: &str, dir: Option<&str>, command: &[String]) -> anyhow::Result<()> {
    let list: Value = serde_json::from_str(&socket::ask_ok(&["list"])?)?;
    let front = in_front(&list, pane);
    let mut split = vec!["split", "-d", "-t", pane];
    if let Some(d) = dir {
        split.extend(["-c", d]);
    }
    split.push("--");
    split.extend(command.iter().map(String::as_str));
    let new = socket::ask_ok(&split)?.trim().to_string();
    socket::ask_ok(&["kill-pane", "-t", pane])?;
    if front {
        // This process may already be gone with the pane it ran in, which
        // is no matter: the new pane was the only one left to take its place.
        let _ = socket::ask(&["select-pane", "-t", &new]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn worktree_functions_are_defined_and_anything_else_runs_as_it_is() {
        let wt = command_for("wt", &["ABC-1".into(), "--prompt".into(), "it's".into()]);
        let line = &wt[3];
        assert!(line.contains("worktree-from-ticket init"), "{line}");
        assert!(line.contains("wt \"$@\""), "{line}");
        assert_eq!(&wt[4..], ["wt", "ABC-1", "--prompt", "it's"]);
        let claude = command_for("claude", &["--resume".into(), "abc".into()]);
        assert!(claude[3].starts_with("\"$@\"; exec "), "{}", claude[3]);
        assert_eq!(&claude[4..], ["claude", "claude", "--resume", "abc"]);
    }

    #[test]
    fn the_pane_in_front_is_read_from_the_list() {
        let list = json!({ "sessions": [{ "tabs": [{ "panes": [
            { "id": "%1", "active": false },
            { "id": "%2", "active": true }
        ] }] }] });
        assert!(in_front(&list, "%2"));
        assert!(!in_front(&list, "%1"));
        assert!(!in_front(&list, "%9"));
    }
}
