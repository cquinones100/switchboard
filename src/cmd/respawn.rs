//! `switchboard respawn [-t pane] [-c dir] <command> [args...]`: the pane's
//! program replaced by another, in the same place, as the skills used
//! `tmux respawn-pane -k` to hand a tab over: /wt to the worktree
//! command that opens the work, and queue-and-clean to `claude --resume` in
//! the main clone once the worktree it ran in is gone.
//!
//! `wt`, `wtb` and `wtpr` run as the worktree commands' shell
//! functions, defined from their `init` in a shell of yours
//! (`worktree::tab_running`), so they work whether or not your rc defines
//! them; anything else runs as it is, in an interactive shell that reads
//! your rc for the PATH. Either way a shell is left in the pane when it
//! ends, in whatever directory it left: the worktree's, after `wt`. The
//! arguments are words of their own, never read as shell, so a prompt or a
//! note's path can hold anything, where the tmux version passed them in the
//! environment to keep them out of the command line.
//!
//! The server's `respawn-program` does it, building the command itself
//! (`command_for`) and respawning the pane as `respawn-pane -k` does
//! (src/mux/respawn.rs): the pane keeps its id, its place, its tab's options
//! and name, and the focus if it had it. Ending the pane's program ends the
//! Claude session that asked for this, so it is the last thing that session
//! runs. Since Claude may run this without asking, the server holds one run
//! from a pane to its own tab and to the programs `allowed` names (wt,
//! wtb, and `claude --resume <id>`), saying why it refuses anything else; a
//! server too old to know `respawn-program` is asked `respawn-pane` with
//! the command built here, as before.

use std::process::ExitCode;

use crate::mux::socket;
use crate::worktree;

/// The worktree commands' shell functions, and the programs that define
/// them.
const FUNCTIONS: &[(&str, &str)] = &[
    ("wt", "worktree-from-ticket"),
    ("wtb", "worktree-from-branch"),
    ("wtpr", "worktree-from-pr"),
];

/// What a respawn from a pane may start, as the refusal names it.
pub const ALLOWED: &str = "wt, wtb or claude --resume <session>";

/// The worktree commands' own flags, naming a file each writes for the
/// shell function that runs it (`worktree::Given`), which their function
/// passes itself and a later one given would take the place of.
const WRITES_FILE: &[&str] = &["--cd-file", "--settings-file"];

/// Whether a respawn from a pane may start `program` with `args`: the
/// worktree commands that hand a tab over (/wt, `wtb`), without the
/// flags that name a file they write, and Claude resuming a session
/// (queue-and-clean, once the worktree is gone).
pub fn allowed(program: &str, args: &[String]) -> bool {
    match program {
        "wt" | "wtb" => !args.iter().any(|a| WRITES_FILE.contains(&a.as_str())),
        "claude" => {
            matches!(args, [flag, id] if flag == "--resume" && !id.is_empty() && !id.starts_with('-'))
        }
        _ => false,
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "Usage: switchboard respawn [-t pane] [-c dir] <command> [args...]\n\n\
         Replaces the pane's program (this pane's, without -t) with <command>,\n\
         started in <dir>. wt, wtb and wtpr run as the worktree commands'\n\
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
    match respawn(&pane, dir.as_deref(), program, program_args) {
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

/// The server's arguments for `respawn-program`: the pane, the directory
/// and the program with its arguments, after `--`.
pub fn program_args(pane: &str, dir: Option<&str>, program: &str, args: &[String]) -> Vec<String> {
    let mut words = vec!["respawn-program".to_string(), "-t".into(), pane.into()];
    if let Some(d) = dir {
        words.extend(["-c".to_string(), d.to_string()]);
    }
    words.push("--".into());
    words.push(program.into());
    words.extend(args.iter().cloned());
    words
}

fn respawn(pane: &str, dir: Option<&str>, program: &str, args: &[String]) -> anyhow::Result<()> {
    // This process may be gone with the program it ran under before the
    // answer comes, which is no matter: the new program is running.
    let words = program_args(pane, dir, program, args);
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let (code, _, stderr) = socket::ask(&words)?;
    if code == 0 {
        return Ok(());
    }
    if !stderr.contains("no command named respawn-program") {
        anyhow::bail!(
            "{}",
            stderr.trim().lines().next().unwrap_or("the server refused")
        );
    }
    // A server from before `respawn-program`, which checks nothing.
    let words = server_args(pane, dir, &command_for(program, args));
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    socket::ask_ok(&words)?;
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
    fn a_pane_may_start_the_worktree_commands_and_resume_claude_alone() {
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(allowed("wt", &words(&["--item", "12", "--note", "N.md"])));
        assert!(allowed("wt", &words(&["ABC-1"])));
        assert!(allowed("wtb", &[]));
        assert!(!allowed(
            "wtb",
            &words(&["main", "--cd-file", "/home/someone/.zshrc"])
        ));
        assert!(!allowed("wt", &words(&["--settings-file", "/tmp/x"])));
        assert!(allowed("claude", &words(&["--resume", "abc"])));
        assert!(!allowed("claude", &[]));
        assert!(!allowed("claude", &words(&["--resume", "abc", "-p", "rm"])));
        assert!(!allowed(
            "claude",
            &words(&["--resume", "--dangerously-skip-permissions"])
        ));
        assert!(!allowed("wtpr", &words(&["12"])));
        assert!(!allowed("sh", &words(&["-c", "id"])));
        assert_eq!(
            program_args("%3", Some("/w"), "wt", &words(&["--item", "1"])),
            [
                "respawn-program",
                "-t",
                "%3",
                "-c",
                "/w",
                "--",
                "wt",
                "--item",
                "1"
            ]
        );
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
