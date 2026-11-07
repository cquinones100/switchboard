//! Every program Switchboard has, by name. `switchboard <name> [args]` runs
//! one; alone, `switchboard` is the workspace. A link to the binary named
//! after a program runs it too, so `wt-jump` and the rest keep their names.
//! What used to be a program run inside tmux is part of the workspace now
//! (the sidebar, search), or a program of its own the workspace runs in a
//! pane (`view`, `pr-view`, `ticket-view`, `link-view`, src/views; `diff`,
//! src/diff; `notes`, src/notes).

use std::process::ExitCode;

pub mod claude_here;
pub mod claude_reply;
pub mod codex_review;
pub mod land_lock;
pub mod platform;
pub mod respawn;
pub mod setting;
pub mod shape;
pub mod shape_batch;
pub mod skill_path;
pub mod slack_relay;
pub mod slack_updates_save;
pub mod statusline;
pub mod tabs;
pub mod today_save;
pub mod update_main;
pub mod wait_deploy;
pub mod watch_note;
pub mod work;
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
        name: "setup",
        run: crate::setup::main,
        about: "Set Claude Code up for Switchboard: its hooks, statusline, CLAUDE.md and skills",
    },
    Command {
        name: "update",
        run: crate::update::main,
        about: "Install the newest release of Switchboard, then its setup (--check only says whether there is one)",
    },
    Command {
        name: "statusline",
        run: statusline::main,
        about: "Claude Code's statusline, which keeps each session's context and spend (run by Claude Code)",
    },
    Command {
        name: "tabs",
        run: tabs::main,
        about: "A line per pane of every tab: its number, place, name, Claude's state, program and directory",
    },
    Command {
        name: "respawn",
        run: respawn::main,
        about: "Replace a pane's program with another, as /wt hands a tab over",
    },
    Command {
        name: "setting",
        run: setting::main,
        about: "One of the workspace's settings, as Settings reads it",
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
    Command {
        name: "view",
        run: crate::views::view_main,
        about: "Page a Markdown file, drawn as the PR view draws one",
    },
    Command {
        name: "pr-view",
        run: crate::views::pr_view_main,
        about: "The branch's PR description, paged (M-v runs it beside a pane)",
    },
    Command {
        name: "ticket-view",
        run: crate::views::ticket_view_main,
        about: "A Linear ticket with its comments, paged (M-i runs it beside a pane)",
    },
    Command {
        name: "link-view",
        run: crate::views::link_view_main,
        about: "What a link holds, read in the terminal",
    },
    Command {
        name: "diff",
        run: crate::diff::main,
        about: "The branch's changes against its PR's base, told as a story (M-d runs it in a pane's place)",
    },
    Command {
        name: "notes",
        run: crate::notes::main,
        about: "The notes app: your notes in $NOTES_DIR or ~/Notes, drawn as Notes shows them and edited with Vim's keys",
    },
    Command {
        name: "open-url",
        run: crate::links::main,
        about: "Open a link in its site's Chrome profile (browser.json)",
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
            println!("switchboard {}", crate::update::version_line());
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
