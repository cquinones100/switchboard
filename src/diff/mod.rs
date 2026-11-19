//! M-d's diff in the workspace: the server's side of it. The diff itself
//! is a crate of its own, `switchboard-diff` (crates/diff), an app that
//! runs alone in any terminal and the first plugin on Switchboard's plugin
//! API (`switchboard-plugin-api`): it never depends on this crate, and
//! reaches the workspace only through the API's `Host`. What is here is
//! what loads it and answers it, ported from the scripts around M-d
//! (tmux/pane-in-place.sh, lib/sidebar/story-ahead.js).
//!
//! Under tmux, M-d ran a stripped-down Neovim in a pane's place; here it is
//! `switchboard diff`, which the server runs in the pane's place
//! (`in_place`), the pane parked until the diff ends, and which loads the
//! plugin in its own process (`main`).
//!
//! - `host`: this program answering the diff for the workspace.
//! - `ahead`: which tab's new commit the diff is handed to tell ahead.
//! - `in_place`: a tool in a pane's place, in the server.
//! - `serve`: the server's keys: M-d, lazygit in a pane's place, and the
//!   palette's commands.
//!
//! The diff's own modules are given at the paths they had (`diff::git`,
//! `diff::story`, `diff::tell` and the rest), for the parts that read a
//! change or a story themselves (`switchboard diff-story`, the keys list).

use std::path::PathBuf;
use std::process::ExitCode;

pub mod ahead;
pub mod host;
pub mod in_place;
mod serve;

pub use switchboard_diff::{app, ask, clear, git, highlight, order, sides, story, tell};
use switchboard_plugin_api::Plugin;

/// What the diff's keys do in the server, an action of the multiplexer's
/// (`mux::keys::Action::Diff`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    /// M-d: the branch's diff in place of this pane, or closed.
    Diff,
    /// The pane a tool stands in for beside it, on its left.
    Side,
    /// The tool alone again, that pane parked once more.
    Full,
    /// yS pressed in the diff: its story copied as Markdown.
    CopyStory,
    /// ga pressed in the diff: a question about it.
    Ask,
    /// lazygit in place of this pane, or quit.
    Lazygit,
    /// The working tree's changes in place of this pane, or closed.
    WorkingTree,
    /// What M-d keeps of the tab's branch removed, after a yes.
    ClearCache,
}

impl Command {
    pub const ALL: [Command; 8] = [
        Command::Diff,
        Command::Side,
        Command::Full,
        Command::CopyStory,
        Command::Ask,
        Command::Lazygit,
        Command::WorkingTree,
        Command::ClearCache,
    ];

    /// Its name in keys.json.
    pub fn name(self) -> &'static str {
        match self {
            Command::Diff => "diff",
            Command::Side => "tool-beside",
            Command::Full => "tool-alone",
            Command::CopyStory => "copy-diff-story",
            Command::Ask => "ask-about-diff",
            Command::Lazygit => "lazygit",
            Command::WorkingTree => "diff-working-tree",
            Command::ClearCache => "clear-diff-cache",
        }
    }

    pub fn parse(name: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.name() == name)
    }

    /// What it does, as tmux.conf described its binding.
    pub fn about(self) -> &'static str {
        match self {
            Command::Diff => {
                "Open the branch's diff against its PR's base in place of this pane, or close it"
            }
            Command::Side => {
                "Show the pane a tool stands in for (the diff, lazygit) beside it, on its left"
            }
            Command::Full => "Show a tool in place of a pane alone again, full width",
            Command::CopyStory => "Copy the diff's story (M-d) as Markdown, to share it",
            Command::Ask => {
                "Ask Claude about the diff (M-d), with its story, reading the repository as needed"
            }
            Command::Lazygit => {
                "Open lazygit in place of this pane, or quit it; quitting brings the pane back"
            }
            Command::WorkingTree => {
                "Open the changes not yet committed, staged and not, in place of this pane, or close them"
            }
            Command::ClearCache => {
                "Clear this tab's diff cache (M-d): the story, file order and PR base kept for its branch"
            }
        }
    }

    /// More on what it does than `about`, for the palette's preview of the chosen command.
    pub fn detail(self) -> &'static str {
        match self {
            Command::Diff => {
                "The base is the PR's base branch, else origin's default, fetched first. With Settings' \"Uncommitted changes in M-d\" on, it runs to the working tree. With nothing to show, a box says so and the pane stays."
            }
            Command::Side => {
                "The parked pane comes back on the tool's left at half the width, so a Claude session and its diff sit side by side. Listed only where a tool stands in for a pane; M-d in either pane closes the diff."
            }
            Command::Full => {
                "The pane brought beside the tool is parked again, its program still running, and takes its whole place back when the tool closes. Listed only where a tool stands in for a pane."
            }
            Command::CopyStory => {
                "It presses yS in the diff: each page's number, title and why, its files, and each change's note as path:line. Listed only while the diff is open; with no story told yet, it says so."
            }
            Command::Ask => {
                "It presses ga in the diff. Haiku runs in the repository with tools to read and search only, keeps no session, and streams its answer beside the diff. Sends the diff and your question to Claude; diff_ask turns it off."
            }
            Command::Lazygit => {
                "It runs in the pane's directory, the pane parked behind it, and quits with its own Ctrl-c rather than q, which would land in a commit message. lazygit isn't shipped with Switchboard; without it, a box says so."
            }
            Command::WorkingTree => {
                "Unstaged changes are listed under Changes and staged ones under Staged changes, with the diff's own keys. Its story reads the staged changes first, with no commit messages; M-d closes it too."
            }
            Command::ClearCache => {
                "It works from any pane of the tab, the diff open or not, so the next M-d asks Haiku again. A story being told right now is kept, and a box says how many files went or why the change couldn't be read."
            }
        }
    }

    /// tmux.conf's tags. `#diffview` lists a command in the palette only
    /// while the pane has the diff in it.
    pub fn tags(self) -> &'static [&'static str] {
        match self {
            Command::Diff => &["git", "diff", "review"],
            Command::Side | Command::Full => &["panes", "split"],
            Command::CopyStory => &["git", "diff", "review", "story", "diffview"],
            Command::Ask => &["git", "diff", "review", "story", "ai", "diffview"],
            Command::Lazygit => &["git", "commit", "stage"],
            Command::WorkingTree => &["git", "diff", "review", "stage"],
            Command::ClearCache => &["git", "diff", "story", "cache"],
        }
    }

    /// The key tmux.conf bound it to, or `None` for one only in the palette.
    pub fn key(self) -> Option<&'static str> {
        match self {
            Command::Diff => Some("M-d"),
            _ => None,
        }
    }
}

/// The diff, as the plugin it is.
pub fn plugin() -> &'static dyn Plugin {
    &switchboard_diff::Diff
}

/// `switchboard diff [--base <ref> | --working] [--session <id>
/// --session-dir <dir>] [--resize-key <key> <columns>]... [dir]`: the diff run on this terminal, loaded as a
/// plugin (a build can't load one built for a newer protocol) with this
/// program answering for the workspace (`host`). `switchboard diff --ahead
/// [--plan <note>] [dir]` hands it a tab's new commit instead (`ahead`):
/// its story told and kept for M-d, with nothing drawn, the tab's plan note
/// (a path in the notes directory) its context.
pub fn main(args: Vec<String>) -> ExitCode {
    if let Err(why) = plugin().manifest().check() {
        eprintln!("switchboard diff: {why}");
        return ExitCode::FAILURE;
    }
    if !args.iter().any(|a| a == "--ahead" || a == "--plan") {
        return plugin().run("switchboard diff", args, host::core());
    }
    let mut plan = String::new();
    let mut ahead = false;
    let mut dir = None;
    let mut rest = args.into_iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--ahead" => ahead = true,
            "--plan" => match rest.next() {
                Some(p) => plan = p,
                None => return usage(),
            },
            _ if arg.starts_with('-') => return usage(),
            _ => dir = Some(PathBuf::from(arg)),
        }
    }
    if !ahead {
        return usage();
    }
    let dir = dir
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| crate::paths::home()));
    ahead::main(&dir, &plan)
}

fn usage() -> ExitCode {
    eprintln!(
        "Usage: switchboard diff [--base <ref> | --working] [--ahead [--plan <note>]] [--session <id> --session-dir <dir>] [--resize-key <key> <columns>]... [dir]"
    );
    ExitCode::from(2)
}
