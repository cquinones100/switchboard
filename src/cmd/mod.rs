//! Every program Switchboard has, by name. `switchboard <name> [args]` runs
//! one; alone, `switchboard` is the workspace. A link to the binary named
//! after a program runs it too, so `wt-jump` and the rest keep their names.
//! What used to be a program run inside tmux (the sidebar, the PR and ticket
//! views, search) is part of the workspace now, not a command.

use std::process::ExitCode;

pub mod claude_here;
pub mod claude_reply;
pub mod codex_review;
pub mod land_lock;
pub mod platform;
pub mod shape;
pub mod shape_batch;
pub mod skill_path;
pub mod slack_relay;
pub mod slack_updates_save;
pub mod today_save;
pub mod update_main;
pub mod wait_deploy;
pub mod watch_note;
pub mod workspace;
pub mod worktree_done;
pub mod worktree_from_branch;
pub mod worktree_from_note;
pub mod worktree_from_pr;
pub mod worktree_from_ticket;
pub mod worktree_resume;
pub mod wt_batch;
pub mod wt_jump;
pub mod wt_revive;

/// A program: its name, what runs it, and a line about it for `switchboard help`.
pub struct Command {
    pub name: &'static str,
    pub run: fn(Vec<String>) -> ExitCode,
    pub about: &'static str,
}

pub const COMMANDS: &[Command] = &[
    Command {
        name: "server",
        run: crate::mux::server::main,
        about: "The server every client attaches to (started for you)",
    },
    Command {
        name: "hook",
        run: crate::agent::hook::main,
        about: "Tell the workspace what a Claude session is doing (run by Claude Code's hooks)",
    },
    Command {
        name: "platform",
        run: platform::main,
        about: "What differs between macOS and Linux, for the shell scripts",
    },
    Command {
        name: "claude-here",
        run: claude_here::main,
        about: "Start or resume Claude in a directory",
    },
    Command {
        name: "claude-reply",
        run: claude_reply::main,
        about: "Read a reply for a Claude pane",
    },
    Command {
        name: "codex-review",
        run: codex_review::main,
        about: "Review a branch's changes with Codex (or Claude)",
    },
    Command {
        name: "shape",
        run: shape::main,
        about: "Start Claude with /shape",
    },
    Command {
        name: "shape-batch",
        run: shape_batch::main,
        about: "Hand /shape a note's checklist items",
    },
    Command {
        name: "skill-path",
        run: skill_path::main,
        about: "The SKILL.md to follow for a skill here",
    },
    Command {
        name: "slack-relay",
        run: slack_relay::main,
        about: "The Slack relay",
    },
    Command {
        name: "update-main",
        run: update_main::main,
        about: "Bring the main clone's default branch up to date",
    },
    Command {
        name: "worktree-done",
        run: worktree_done::main,
        about: "Finish with a worktree",
    },
    Command {
        name: "worktree-from-branch",
        run: worktree_from_branch::main,
        about: "A worktree for a branch",
    },
    Command {
        name: "worktree-from-note",
        run: worktree_from_note::main,
        about: "A worktree for a note's checklist item",
    },
    Command {
        name: "worktree-from-pr",
        run: worktree_from_pr::main,
        about: "A worktree for a pull request",
    },
    Command {
        name: "worktree-from-ticket",
        run: worktree_from_ticket::main,
        about: "A worktree for a Linear ticket",
    },
    Command {
        name: "worktree-resume",
        run: worktree_resume::main,
        about: "Resume work in a worktree",
    },
    Command {
        name: "wt-batch",
        run: wt_batch::main,
        about: "Open several tickets' worktrees",
    },
    Command {
        name: "wt-jump",
        run: wt_jump::main,
        about: "Jump to a ticket's, PR's or branch's tab",
    },
    Command {
        name: "wt-revive",
        run: wt_revive::main,
        about: "The worktrees no tab has open",
    },
    Command {
        name: "land-lock",
        run: land_lock::main,
        about: "The landing lock and queue",
    },
    Command {
        name: "wait-deploy",
        run: wait_deploy::main,
        about: "Wait for a commit's deploy",
    },
    Command {
        name: "watch-note",
        run: watch_note::main,
        about: "Watch a note for new items",
    },
    Command {
        name: "slack-updates-save",
        run: slack_updates_save::main,
        about: "Save what /slack-updates read",
    },
    Command {
        name: "today-save",
        run: today_save::main,
        about: "Save the day /today read",
    },
];

/// Runs the program `name` with `args`; the workspace with no name.
pub fn run(name: Option<&str>, args: Vec<String>) -> ExitCode {
    match name {
        None => workspace::main(args),
        Some("help" | "-h" | "--help") => {
            help();
            ExitCode::SUCCESS
        }
        Some("version" | "-V" | "--version") => {
            println!("switchboard {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(name) if crate::mux::commands::NAMES.contains(&name) => {
            crate::mux::socket::run_command(std::iter::once(name.to_string()).chain(args).collect())
        }
        Some(name) => match COMMANDS.iter().find(|c| c.name == name) {
            Some(command) => (command.run)(args),
            None => {
                eprintln!("switchboard: no command named {name} (switchboard help lists them)");
                ExitCode::from(2)
            }
        },
    }
}

fn help() {
    println!("Usage: switchboard              start the workspace, or attach to it");
    println!("       switchboard <command> ...  run one of its programs:\n");
    let width = COMMANDS.iter().map(|c| c.name.len()).max().unwrap_or(0);
    for c in COMMANDS {
        println!("  {:width$}  {}", c.name, c.about);
    }
    println!(
        "\nAnd, asked of the running server (switchboard <command> --help-less; see src/mux/commands.rs):"
    );
    println!("  {}", crate::mux::commands::NAMES.join(" "));
}

/// What a program not yet ported from Node says.
pub fn not_yet(name: &str) -> ExitCode {
    eprintln!("switchboard: {name} isn't ported to Rust yet");
    ExitCode::from(70)
}
