//! M-d's diff: a branch's changes as GitHub's "Files changed" shows them,
//! ported from diffview.nvim as M-d set it up (nvim/lua/plugins/diffview.lua,
//! nvim/lua/diffstory.lua, nvim/lua/diffask.lua, nvim/diff.lua) and the
//! scripts around it (tmux/diff-base.sh, tmux/diff-story-ahead.sh,
//! lib/sidebar/story-ahead.js).
//!
//! Under tmux, M-d ran a stripped-down Neovim in a pane's place; here it is
//! `switchboard diff`, a program of Switchboard's own run in a pane.
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

use std::path::PathBuf;
use std::process::ExitCode;

pub mod ahead;
pub mod app;
pub mod ask;
pub mod git;
pub mod highlight;
pub mod order;
pub mod sides;
pub mod story;
pub mod tell;

/// `switchboard diff [--base <ref>] [--ahead] [dir]`: the change in `dir`
/// (where it runs, else) against `<ref>`, or, without one, its PR's base as
/// M-d works it out; with `--ahead`, its story told and kept for M-d, with
/// nothing drawn.
pub fn main(args: Vec<String>) -> ExitCode {
    let mut base = None;
    let mut ahead = false;
    let mut dir = None;
    let mut rest = args.into_iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--base" => match rest.next() {
                Some(b) if git::safe_ref(&b) => base = Some(b),
                _ => return usage(),
            },
            "--ahead" => ahead = true,
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
    if ahead {
        return ahead::main(&dir);
    }
    app::run(dir, base)
}

fn usage() -> ExitCode {
    eprintln!("Usage: switchboard diff [--base <ref>] [--ahead] [dir]");
    ExitCode::from(2)
}
