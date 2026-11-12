//! `switchboard diff-story`: the story M-d tells of the pane's branch, as
//! Markdown, so the Claude in a tab can read the story you are looking at
//! (Switchboard's part of Claude's instructions names it). New with the port:
//! the tmux version kept its stories in Neovim, where no command reached them.
//!
//! It reads the same change M-d's story is told of (the branch's commits
//! against its PR's base, `git::read_change`), takes its key as M-d and the
//! story told ahead do (`story::key_of`) and prints the story kept under it
//! in the cache's `diff-story/`, as `yS` in the diff copies it
//! (`story::markdown`), with the files changed only since the last commit
//! in a last "Not committed yet" chapter, as M-d shows them while the
//! diff_uncommitted setting is on (`story::with_uncommitted`). It only reads: the base is the one M-d kept for the
//! branch's commit (`git::cached_base`), never fetched or asked of GitHub,
//! and no story is ever asked of Claude, so with no base kept or no story
//! told it says so and exits 0. `--path` prints the story's file instead,
//! whether or not it is there yet.
//!
//! The directory is the pane's as the workspace has it (`display cwd`, which
//! follows a tab's `@tab_dir`, as M-d does), else the current one.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::diff::{app, git, order, story, tell};
use crate::mux::socket;
use crate::{settings, util};

/// What is said when the change has no story kept.
pub const UNTOLD: &str = "No story told for this change yet: open M-d and press its story key.";
/// What is said while M-d tells no story (Settings' diff_story or
/// diff_order off).
pub const STORY_OFF: &str = "M-d tells no story while Settings' \"Diff told as a story\" (diff_story) or \"Files in reading order\" (diff_order) is off.";

/// `switchboard diff-story [--path] [dir]`.
pub fn main(args: Vec<String>) -> ExitCode {
    let mut path_only = false;
    let mut dir = None;
    for arg in args {
        match arg.as_str() {
            "--path" => path_only = true,
            "-h" | "--help" => {
                println!(
                    "Usage: switchboard diff-story [--path] [dir]\n\
                     Prints the story M-d tells of the branch's change, as Markdown; --path prints its file."
                );
                return ExitCode::SUCCESS;
            }
            _ if arg.starts_with('-') => return usage(),
            _ if dir.is_none() => dir = Some(PathBuf::from(arg)),
            _ => return usage(),
        }
    }
    let dir = dir.unwrap_or_else(pane_dir);
    if !path_only && !(settings::is_on("diff_order") && settings::is_on("diff_story")) {
        println!("{STORY_OFF}");
        return ExitCode::SUCCESS;
    }
    match read(&dir) {
        Err(why) => {
            eprintln!("switchboard diff-story: {why}");
            ExitCode::from(1)
        }
        Ok(Found::NoBase) if path_only => {
            eprintln!(
                "switchboard diff-story: no base kept for this commit, so no story file yet: open M-d"
            );
            ExitCode::from(1)
        }
        Ok(Found::NoBase) => {
            println!("{UNTOLD}");
            ExitCode::SUCCESS
        }
        Ok(Found::Change { file, .. }) if path_only => {
            println!("{}", file.display());
            ExitCode::SUCCESS
        }
        Ok(Found::Change { story, .. }) => {
            match story {
                Some(text) => print!("{text}"),
                None => println!("{UNTOLD}"),
            }
            ExitCode::SUCCESS
        }
    }
}

fn usage() -> ExitCode {
    eprintln!("Usage: switchboard diff-story [--path] [dir]");
    ExitCode::from(2)
}

/// The pane's directory as the workspace has it, else the current one.
fn pane_dir() -> PathBuf {
    std::env::var_os("SWITCHBOARD_PANE")
        .and_then(|_| socket::ask_ok(&["display", "cwd"]).ok())
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| crate::paths::home()))
}

/// What was found of `dir`'s change.
enum Found {
    /// M-d kept no base for the branch's commit, so no story can be kept.
    NoBase,
    /// The story's file, and the story as Markdown when one is kept.
    Change {
        file: PathBuf,
        story: Option<String>,
    },
}

fn read(dir: &Path) -> Result<Found, String> {
    let facts = git::facts(dir).map_err(|_| "not in a git repository".to_string())?;
    let Some(base) = git::cached_base(dir) else {
        return Ok(Found::NoBase);
    };
    let whole = if settings::is_on("diff_uncommitted") {
        git::read_to_work_tree(&facts.top, &base)
    } else {
        git::read_change(&facts.top, &base)
    }
    .map_err(|e| e.to_string())?;
    let change = whole.told_part();
    let (key, paths) = story::key_of(change);
    let file = tell::story_dir().join(format!("{key}.txt"));
    let (_, hunks) = story::split(&[change.diff.as_str()]);
    let told = util::read_text(&file)
        .and_then(|text| story::parse(&text, &paths, order::generated, &hunks).ok());
    // The files changed only since the last commit in a last chapter, as
    // M-d shows them (`app::show_story`): after the story told, else after
    // the commits' files in a "Committed" chapter.
    let (dirty, committed) = app::uncommitted_orders(&whole);
    let shown = if dirty.is_empty() {
        told
    } else {
        Some(story::with_uncommitted(told.as_ref(), &dirty, &committed))
    };
    Ok(Found::Change {
        file,
        story: shown.map(|s| story::markdown(&s)),
    })
}
