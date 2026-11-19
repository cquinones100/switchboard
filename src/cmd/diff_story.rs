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
//! in a last "Not committed yet" page, as M-d shows them while the
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

/// How wide --html draws the Story view: room for the outline and a
/// column wide enough for its changes side by side.
const PAGE_COLS: u16 = 170;

/// The diff's options as M-d reads them from the settings, for --html.
fn page_options() -> app::Options {
    let on = settings::is_on;
    app::Options {
        order: true,
        story: true,
        pages_alone: on("diff_story_pages"),
        progress: false,
        notes: on("diff_story_notes"),
        list_wide: on("diff_list_wide"),
        uncommitted: on("diff_uncommitted"),
        ask: false,
        easy: false,
        key_help: false,
        ux2: settings::is_on("ux2"),
        hover: false,
        light: false,
        pane_arrows: false,
        haiku: false,
        server: false,
        clipboard: false,
        scroll_bars: false,
        alone: true,
        map: true,
        keep: false,
        menu_mouse: false,
        pointer: false,
        resize: false,
    }
}

/// What is said when the change has no story kept.
pub const UNTOLD: &str = "No story told for this change yet: open M-d and press its story key.";
/// What is said while M-d tells no story (Settings' diff_story or
/// diff_order off).
pub const STORY_OFF: &str = "M-d tells no story while Settings' \"Diff told as a story\" (diff_story) or \"Files in reading order\" (diff_order) is off.";

/// `switchboard diff-story [--path] [dir]`.
pub fn main(args: Vec<String>) -> ExitCode {
    let mut path_only = false;
    let mut html = None;
    let mut dir = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--path" => path_only = true,
            "--html" => html = Some(PAGE_COLS),
            "--cols" => match args.next().and_then(|n| n.parse::<u16>().ok()) {
                Some(n) if n >= 80 => html = Some(n),
                _ => return usage(),
            },
            "-h" | "--help" => {
                println!(
                    "Usage: switchboard diff-story [--path | --html [--cols <n>]] [dir]\n\
                     Prints the story M-d tells of the branch's change, as Markdown; --path prints its file;\n\
                     --html prints M-d's Story view as one HTML page, <n> columns wide (170 unless said)."
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
    match read(&dir, html) {
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
    eprintln!("Usage: switchboard diff-story [--path | --html [--cols <n>]] [dir]");
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

fn read(dir: &Path, html: Option<u16>) -> Result<Found, String> {
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
    // The files changed only since the last commit in a last page, as
    // M-d shows them (`app::show_story`): after the story told, else after
    // the commits' files in a "Committed" page.
    let (dirty, committed) = app::uncommitted_orders(&whole);
    let shown = if dirty.is_empty() {
        told
    } else {
        Some(story::with_uncommitted(told.as_ref(), &dirty, &committed))
    };
    let text = match html {
        // The Story view as a page, drawn by the diff's own code; a story
        // told as a list has none.
        Some(cols) => match shown {
            Some(told) if !told.chapters.is_empty() => {
                let title = told.chapters[0].title.clone();
                app::story_page(page_options(), cols, whole, told, &title)
            }
            _ => None,
        },
        None => shown.map(|s| story::markdown(&s)),
    };
    Ok(Found::Change { file, story: text })
}
