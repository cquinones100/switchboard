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
//! The server's `respawn-pane -k` does it (src/mux/respawn.rs): the pane
//! keeps its id, its place, its tab's options and name, and the focus if it
//! had it. Ending the pane's program ends the Claude session that asked for
//! this, so it is the last thing that session runs.

use std::process::ExitCode;

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

/// The server's arguments for it: the pane, the directory, the program
/// ended whether or not it still runs (as the skills ran `tmux
/// respawn-pane -k`), and the command, after `--` so none of its words is
/// read as a flag.
pub fn server_args(pane: &str, dir: Option<&str>, command: &[String]) -> Vec<String> {
    let mut args = vec![
        "respawn-pane".to_string(),
        "-k".into(),
        "-t".into(),
        pane.into(),
    ];
    if let Some(d) = dir {
        args.extend(["-c".to_string(), d.to_string()]);
    }
    args.push("--".into());
    args.extend(command.iter().cloned());
    args
}

fn respawn(pane: &str, dir: Option<&str>, command: &[String]) -> anyhow::Result<()> {
    let args = server_args(pane, dir, command);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    // This process may be gone with the program it ran under before the
    // answer comes, which is no matter: the new program is running.
    socket::ask_ok(&args)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_server_is_asked_to_end_the_program_and_start_the_command() {
        let args = server_args("%3", Some("/w"), &["sh".into(), "-c".into(), "-x".into()]);
        assert_eq!(
            args,
            [
                "respawn-pane",
                "-k",
                "-t",
                "%3",
                "-c",
                "/w",
                "--",
                "sh",
                "-c",
                "-x"
            ]
        );
        assert_eq!(
            server_args("%3", None, &["claude".into()]),
            ["respawn-pane", "-k", "-t", "%3", "--", "claude"]
        );
    }
}
