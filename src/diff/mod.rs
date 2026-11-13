//! M-d's diff: a branch's changes as GitHub's "Files changed" shows them,
//! in place of the pane you are in, ported from diffview.nvim as M-d set it
//! up (nvim/lua/plugins/diffview.lua, nvim/lua/diffstory.lua,
//! nvim/lua/diffask.lua, nvim/diff.lua) and the scripts around it
//! (tmux/pane-in-place.sh, tmux/diff-base.sh, tmux/diff-story-ahead.sh,
//! lib/sidebar/story-ahead.js).
//!
//! Under tmux, M-d ran a stripped-down Neovim in a pane's place; here it is
//! `switchboard diff`, a program of Switchboard's own the server runs in
//! the pane's place (`in_place`), the pane parked until the diff ends.
//!
//! - `git`: what the branch is compared with, and the change itself.
//! - `order`: the files in reading order, generated files last.
//! - `sides`: one file's two sides lined up, its changed words, its folds.
//! - `highlight`: syntax colours.
//! - `story`: the diff told as a story: what Haiku reads, its answer read.
//! - `tell`: telling it, cached, and followed while another program tells it.
//! - `ask`: a question about the diff, answered by Haiku.
//! - `app`: the program on the screen.
//! - `ahead`: the story told ahead of time for each tab's new commit.
//! - `in_place`: a tool in a pane's place, in the server.
//! - `clear`: M-p's "Clear this tab's diff cache".
//! - `serve`: the server's keys: M-d, lazygit in a pane's place, and the
//!   palette's commands.

use std::path::PathBuf;
use std::process::ExitCode;

pub mod ahead;
pub mod app;
pub mod ask;
pub mod clear;
pub mod git;
pub mod highlight;
pub mod in_place;
pub mod order;
mod serve;
pub mod sides;
pub mod story;
pub mod tell;

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
                "It presses yS in the diff: each chapter's number, title and why, its files, and each change's note as path:line. Listed only while the diff is open; with no story told yet, it says so."
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

/// `switchboard diff [--base <ref> | --working] [--ahead] [--session <id>
/// --session-dir <dir>] [dir]`: the
/// change in `dir` (where it runs, else) against `<ref>`, or, without one,
/// its PR's base as M-d works it out, to the working tree with the
/// diff_uncommitted setting on; with `--working`, the working tree's
/// changes, staged and not; with `--ahead`, its story told and kept for
/// M-d, with nothing drawn; with `--session`, `ga` asks a copy of that
/// Claude session (the tab's, which M-d passes), resumed from the directory
/// `--session-dir` names.
pub fn main(args: Vec<String>) -> ExitCode {
    let mut base = None;
    let mut working = false;
    let mut ahead = false;
    let mut dir = None;
    let (mut session, mut session_dir) = (None, None);
    let mut rest = args.into_iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--base" => match rest.next() {
                Some(b) if git::safe_ref(&b) => base = Some(b),
                _ => return usage(),
            },
            "--ahead" => ahead = true,
            "--session" => match rest.next() {
                Some(id) => session = Some(id),
                None => return usage(),
            },
            "--session-dir" => match rest.next() {
                Some(d) => session_dir = Some(d),
                None => return usage(),
            },
            "--working" => working = true,
            "-h" | "--help" => {
                usage();
                return ExitCode::SUCCESS;
            }
            _ if arg.starts_with('-') => return usage(),
            _ => dir = Some(PathBuf::from(arg)),
        }
    }
    let dir = dir
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| crate::paths::home()));
    if (working && (ahead || base.is_some())) || (ahead && base.is_some()) {
        return usage();
    }
    if ahead {
        return ahead::main(&dir);
    }
    let session = match (session, session_dir) {
        (None, None) => None,
        (Some(id), Some(at)) => match ask::Session::new(&id, &at) {
            Some(s) => Some(s),
            None => return usage(),
        },
        _ => return usage(),
    };
    app::run(dir, base, working, session)
}

fn usage() -> ExitCode {
    eprintln!(
        "Usage: switchboard diff [--base <ref> | --working] [--ahead] [--session <id> --session-dir <dir>] [dir]"
    );
    ExitCode::from(2)
}
