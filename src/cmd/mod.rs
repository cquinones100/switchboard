//! Every program Switchboard has, by name. `switchboard <name> [args]` runs
//! one; alone, `switchboard` is the workspace. A link to the binary named
//! after a program runs it too, so `wt-jump` and the rest keep their names.
//! What used to be a program run inside tmux is part of the workspace now
//! (the sidebar, search), or a program of its own the workspace runs in a
//! pane (`view`, `pr-view`, `ticket-view`, `link-view`, `pr-image`,
//! src/views; `diff`, src/diff; `notes`, src/notes).

use std::process::ExitCode;

pub mod apply_actions;
pub mod autopilot_duplicate;
pub mod aws_login;
pub mod claude_context;
pub mod claude_here;
pub mod claude_plugin_dirs;
pub mod claude_reply;
pub mod clone_build;
pub mod codex_review;
pub mod coordinator;
pub mod demo;
pub mod diff_story;
pub mod headless;
pub mod item_done;
pub mod land_lock;
pub mod land_push;
pub mod land_settle;
pub mod land_squash;
pub mod land_test;
pub mod landing;
pub mod leak_check;
pub mod platform;
pub mod pr_risk;
pub mod respawn;
pub mod setting;
pub mod shape;
pub mod shape_batch;
pub mod shape_note;
pub mod skill_path;
pub mod slack_relay;
pub mod slack_updates_save;
pub mod slot;
pub mod statusline;
pub mod tabs;
pub mod tickets;
pub mod today_save;
pub mod try_;
pub mod try_sealed;
pub mod update_main;
pub mod update_target;
pub mod wait_ci;
pub mod wait_deploy;
pub mod watch_note;
pub mod work;
pub mod work_item_box;
pub mod work_recent;
pub mod workspace;
pub mod worktree_done;
pub mod worktree_from_branch;
pub mod worktree_from_issue;
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
        about: "Set Claude Code up for Switchboard: its plugin (hooks, skills, instructions), statusline, permissions and CLAUDE.md",
    },
    Command {
        name: "try",
        run: try_::main,
        about: "Build a Switchboard worktree and run it under a home of its own (--detached, --print, --clean; --sealed for someone else's code, in a sandbox)",
    },
    Command {
        name: "demo",
        run: demo::main,
        about: "The workspace on made-up data, in a home of its own that links nothing of yours and reaches no service (--detached, --print, --reset, --clean, --tour <dir>)",
    },
    Command {
        name: "demo-services",
        run: demo::services_main,
        about: "The demo's stand-ins for GitHub, Linear and Slack (run by switchboard demo)",
    },
    Command {
        name: "demo-stand-in",
        run: crate::demo::stand_in::main,
        about: "One of the demo's stand-in programs: claude, gh, the openers, the clipboard (run by the demo's scripts)",
    },
    Command {
        name: "headless",
        run: headless::main,
        about: "Attach a client with no terminal and print its id, for headless-keys and headless-screen (--size, --min-size, --session, --stop <id>)",
    },
    Command {
        name: "headless-keys",
        run: headless::keys_main,
        about: "Type keys by name into a headless client, as a terminal would send them",
    },
    Command {
        name: "headless-screen",
        run: headless::screen_main,
        about: "A headless client's whole screen, as text, --ansi with its colors, or --png <file>",
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
        name: "claude-context",
        run: claude_context::main,
        about: "Switchboard's instructions for Claude, as a session starts (run by Claude Code's hooks)",
    },
    Command {
        name: "coordinator-order",
        run: coordinator::order_main,
        about: "The coordinator setting the order autopilot takes issues in: those named priority 1 and first in its queue in that order, those after --left priority 4 and out of it, on repositories you can push to",
    },
    Command {
        name: "coordinator-post",
        run: coordinator::post_main,
        about: "The coordinator filing an issue or a comment on its clone's GitHub repository (one you can push to), its body from its posts folder, after a clean leak-check",
    },
    Command {
        name: "tabs",
        run: tabs::main,
        about: "A line per pane of every tab: its number, place, name, Claude's state, program and directory, then its Claude's session, pid, name and address (--json for objects)",
    },
    Command {
        name: "diff-story",
        run: diff_story::main,
        about: "The story M-d tells of the branch's change, as Markdown, from what it kept (--path for its file)",
    },
    Command {
        name: "respawn",
        run: respawn::main,
        about: "Replace a pane's program with another, as /wt hands a tab over",
    },
    Command {
        name: "setting",
        run: setting::main,
        about: "One of the workspace's settings, as Settings reads it; with a value (or --reset), changes it",
    },
    Command {
        name: "platform",
        run: platform::main,
        about: "What differs between macOS and Linux, for the shell scripts",
    },
    Command {
        name: "claude-here",
        run: claude_here::main,
        about: "Resume the last Claude session of a directory (a picker when there are several)",
    },
    Command {
        name: "claude-plugin-dirs",
        run: claude_plugin_dirs::main,
        about: "CLAUDE_CODE_PLUGIN_DIRS for a Claude started now in a pane (the claude function wt's init prints runs it)",
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
        name: "pr-risk",
        run: pr_risk::main,
        about: "What someone else's PR changes that runs without being asked, read from git alone",
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
        name: "phone",
        run: crate::phone::cli::main,
        about: "The phone app: your Claude tabs answered from your phone, on your tailnet alone",
    },
    Command {
        name: "update-main",
        run: update_main::main,
        about: "Bring the main clone's default branch up to date",
    },
    Command {
        name: "update-target",
        run: update_target::main,
        about: "Merge the default branch into a target branch under its landing lock, test it and push it",
    },
    Command {
        name: "clone-build",
        run: clone_build::main,
        about: "Start a new worktree's target/ as a copy-on-write clone of its main clone's (run by the worktree commands)",
    },
    Command {
        name: "worktree-done",
        run: worktree_done::main,
        about: "Remove the worktrees whose Linear ticket is done or canceled, picked from a list",
    },
    Command {
        name: "worktree-from-branch",
        run: worktree_from_branch::main,
        about: "A worktree for a branch",
    },
    Command {
        name: "worktree-from-pr",
        run: worktree_from_pr::main,
        about: "A worktree for a pull request",
    },
    Command {
        name: "worktree-from-ticket",
        run: worktree_from_ticket::main,
        about: "A worktree for a Linear ticket, a GitHub issue or a note's checklist item",
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
        name: "slot",
        run: slot::main,
        about: "Run a command holding one of a kind's slots in the queue for heavy work (build, tests, js), waiting in line for one",
    },
    Command {
        name: "autopilot-duplicate",
        run: autopilot_duplicate::main,
        about: "In a tab autopilot opened, mark its item a duplicate of other work, so autopilot skips it with that reason",
    },
    Command {
        name: "item-done",
        run: item_done::main,
        about: "Tick the item a landed worktree was started for, with its Done line, then remove the worktree and its branch",
    },
    Command {
        name: "landing",
        run: landing::main,
        about: "How the repository here lands its work: through a PR or straight on its default branch, and a direct landing's steps",
    },
    Command {
        name: "tickets",
        run: tickets::main,
        about: "Where the repository here keeps its work (github, linear or notes), which /shape files new work in, and the label autopilot takes its issues by",
    },
    Command {
        name: "leak-check",
        run: leak_check::main,
        about: "What a range of commits adds, or a file holds (--file), that may identify you or your employer, before it goes to a public repository",
    },
    Command {
        name: "apply-actions",
        run: apply_actions::main,
        about: "What a landed change needs to take effect here, one action a line",
    },
    Command {
        name: "apply",
        run: apply_actions::apply_main,
        about: "Put a change to Switchboard into effect: install (build the clone and put it in place) or relay (restart it)",
    },
    Command {
        name: "wait-ci",
        run: wait_ci::main,
        about: "Wait for a commit's CI",
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
        about: "Page a Markdown file, drawn as the PR view draws one, or any file from a line (+N)",
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
        name: "pr-image",
        run: crate::views::image::main,
        about: "The branch's PR description (or --ticket <ID>) as GitHub draws it, as pictures (M-p runs it in a pane's place)",
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
        name: "new-note",
        run: crate::day::new_notes::main,
        about: "Put a note just written for you (its path in your notes) in the notifications until you read it, as the todo note and meeting notes routines do",
    },
    Command {
        name: "notifications",
        run: crate::sidebar::notices::main,
        about: "The Notifications window's pane, which the workspace draws its list over (started for you)",
    },
    Command {
        name: "open-url",
        run: crate::links::main,
        about: "Open a link in its site's Chrome profile (browser.json; --as <site> for another site's)",
    },
    Command {
        name: "aws-login",
        run: aws_login::main,
        about: "Log in to AWS (aws sso login) in your Chrome profile (M-p runs it beside a pane)",
    },
];

/// Runs the program `name` with `args`; the workspace with no name.
/// The programs that say how they are used themselves when asked with
/// `--help`; any other asked so says what it is (its line in `help`)
/// rather than taking `--help` for a file, a link, a tab or a note.
const OWN_HELP: &[&str] = &[
    "phone",
    "demo",
    "diff-story",
    "respawn",
    "setting",
    "platform",
    "codex-review",
    "pr-risk",
    "shape",
    "shape-batch",
    "skill-path",
    "slack-relay",
    "update-main",
    "item-done",
    "autopilot-duplicate",
    "worktree-done",
    "worktree-from-branch",
    "worktree-from-pr",
    "worktree-from-ticket",
    "worktree-resume",
    "wt-batch",
    "wt-jump",
    "wt-revive",
    "land-lock",
    "wait-ci",
    "wait-deploy",
    "diff",
    "notes",
    "notifications",
    "aws-login",
    "try",
    "update",
    "setup",
];

/// The programs whose first word is one of a fixed set of subcommands, and
/// those subcommands, so `skill-path` can tell a skill newer than this build
/// (one naming `land-lock test` to a binary from before it) by what the
/// skill's `allowed-tools` name. Every other program takes any first word.
pub const SUBCOMMANDS: &[(&str, &[&str])] = &[
    (
        "land-lock",
        &[
            "take",
            "wait",
            "push",
            "release",
            "requeue",
            "leave",
            "holder",
            "queue",
            "move",
            "phase",
            "record",
            "squash",
            "test",
            "settle",
            "landed",
            "autolanded",
            "queue-pr",
            "dequeue-pr",
            "move-pr",
            "pr-status",
            "pr-queue",
        ],
    ),
    (
        "slack-relay",
        &[
            "start", "stop", "status", "run", "relay", "notify", "sends", "help",
        ],
    ),
    (
        "phone",
        &[
            "start", "stop", "status", "run", "pair", "devices", "revoke", "preview", "help",
        ],
    ),
];

/// Whether this build has `switchboard <name> <first>`: the program, and,
/// for one in `SUBCOMMANDS`, its subcommand (a first word that is a flag or
/// a wildcard counts as none).
pub fn has(name: &str, first: Option<&str>) -> bool {
    if !is_command(name) {
        return false;
    }
    match (SUBCOMMANDS.iter().find(|(n, _)| *n == name), first) {
        (Some((_, subs)), Some(word)) if !word.starts_with('-') && !word.contains('*') => {
            subs.contains(&word)
        }
        _ => true,
    }
}

/// Whether `name` is a command a link to the binary may be named after: one
/// of its programs, or a command asked of the running server. A file of any
/// other name (`.switchboard.new.123`, which an installer runs once before
/// renaming it into place, or `switchboard-0.2.0`) runs as `switchboard`.
pub fn is_command(name: &str) -> bool {
    crate::mux::commands::NAMES.contains(&name) || COMMANDS.iter().any(|c| c.name == name)
}

pub fn run(name: Option<&str>, args: Vec<String>) -> ExitCode {
    crate::settings::share_with_ui();
    crate::diff::host::install();
    crate::notes::host::install();
    match name {
        None => workspace::main(args),
        Some("help" | "-h" | "--help") => {
            help();
            ExitCode::SUCCESS
        }
        Some("version" | "-V" | "--version") => {
            println!("switchboard {}", crate::update::version_line());
            if let Some(dir) = crate::assets::dir() {
                println!("assets read from {}", crate::paths::tilde(&dir));
            }
            ExitCode::SUCCESS
        }
        Some(name) if crate::mux::commands::NAMES.contains(&name) => {
            crate::mux::socket::run_command(std::iter::once(name.to_string()).chain(args).collect())
        }
        Some(name) => match COMMANDS.iter().find(|c| c.name == name) {
            Some(command)
                if !OWN_HELP.contains(&name)
                    && matches!(args.as_slice(), [a] if a == "--help" || a == "-h") =>
            {
                println!("switchboard {name}: {}", command.about);
                ExitCode::SUCCESS
            }
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
        "\nAnd, asked of the running server (switchboard <command> --help says how each is used):"
    );
    println!("  {}", crate::mux::commands::NAMES.join(" "));
}

/// What a program not yet ported from Node says.
pub fn not_yet(name: &str) -> ExitCode {
    eprintln!("switchboard: {name} isn't ported to Rust yet");
    ExitCode::from(70)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_program_with_its_own_help_is_one_of_the_commands() {
        for name in OWN_HELP {
            assert!(
                COMMANDS.iter().any(|c| c.name == *name),
                "{name} isn't a command"
            );
        }
    }

    #[test]
    fn every_subcommand_listed_is_in_its_programs_usage() {
        let usage_of = |name| match name {
            "land-lock" => include_str!("land_lock.rs"),
            "slack-relay" => include_str!("../relay/cli.rs"),
            "phone" => include_str!("../phone/cli.rs"),
            other => panic!("{other} has no usage to check"),
        };
        for (name, subs) in SUBCOMMANDS {
            let source = usage_of(name);
            for sub in *subs {
                assert!(
                    source.contains(&format!("\"{sub}\"")),
                    "{name} {sub} isn't dispatched"
                );
            }
        }
    }

    #[test]
    fn every_switchboard_command_a_shipped_skill_allows_is_in_this_build() {
        let shipped = crate::setup::skills::shipped()
            .into_iter()
            .filter_map(|(name, files)| {
                let (_, text) = files.into_iter().find(|(path, _)| path == "SKILL.md")?;
                Some((name, String::from_utf8_lossy(&text).into_owned()))
            });
        let own_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".claude/skills");
        let own = std::fs::read_dir(&own_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let text = std::fs::read_to_string(entry.path().join("SKILL.md")).ok()?;
                Some((entry.file_name().to_string_lossy().into_owned(), text))
            });
        let mut checked = 0;
        for (name, text) in shipped.chain(own) {
            let missing = skill_path::missing_commands(&text);
            assert!(missing.is_empty(), "{name} names {missing:?}");
            checked += 1;
        }
        assert!(checked > 10, "only {checked} skills read");
    }

    #[test]
    fn a_subcommand_this_build_lacks_is_missing() {
        assert!(has("land-lock", Some("test")));
        assert!(has("land-lock", Some("--target")));
        assert!(has("land-lock", Some("*")));
        assert!(!has("land-lock", Some("nosuch")));
        assert!(!has("no-such-program", None));
        assert!(has("setting", Some("theme")));
        assert!(has("show", Some("*")));
    }
}
