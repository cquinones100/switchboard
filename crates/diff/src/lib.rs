//! The diff: a branch's changes as GitHub's "Files changed" shows them,
//! side by side in a terminal, told as a story. Ported from diffview.nvim
//! as M-d set it up (nvim/lua/plugins/diffview.lua, nvim/lua/diffstory.lua,
//! nvim/lua/diffask.lua, nvim/diff.lua) and the scripts around it
//! (tmux/diff-base.sh, tmux/diff-story-ahead.sh), and taken out of the
//! `switchboard` crate (src/diff) to be an app of its own and the first
//! plugin on Switchboard's plugin API.
//!
//! It runs two ways, the same program in both:
//!
//! - alone, as `switchboard-diff` (src/main.rs), in any terminal and any
//!   repository, with no server: it answers for the workspace itself
//!   (`host::Alone`);
//! - in the workspace, as `switchboard diff`, which M-d runs in a pane's
//!   place: `switchboard` loads it in its own process through the API's
//!   `Plugin` (`Diff`) and answers what it asks (`Host`).
//!
//! It reaches the workspace only through that API (`host`): its settings,
//! its folders, the pull request's base, whether a directory holds someone
//! else's code, the request audit, the tab's plan note, and the server's
//! commands (saying something in a box, the pane beside, a file opened
//! below). Of the workspace's crates it depends on `switchboard-plugin-api`
//! and `switchboard-ui` alone, so the compiler holds that. What it runs
//! (git, `claude`, your editor) is given its options as it starts, by
//! flags or environment, never by a change to that program's own config.
//!
//! - `git`: what the branch is compared with, and the change itself.
//! - `order`: the files in reading order, generated files last.
//! - `sides`: one file's two sides lined up, its changed words, its folds.
//! - `highlight`: syntax colours.
//! - `story`: the diff told as a story: what Haiku reads, its answer read.
//! - `tell`: telling it, cached, and followed while another program tells it.
//! - `ask`: a question about the diff, answered by Haiku.
//! - `app`: the program on the screen.
//! - `ahead`: a commit's story told ahead of time, the event the workspace
//!   hands it.
//! - `clear`: what it keeps of a branch removed.
//! - `host`: who answers for the workspace.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use serde_json::json;
use switchboard_plugin_api::{Event, Host, Manifest, PROTOCOL_VERSION, Plugin, Setting};

pub mod ahead;
pub mod app;
pub mod ask;
pub mod clear;
pub mod git;
pub mod highlight;
pub mod host;
pub mod order;
pub mod sides;
pub mod story;
pub mod tell;

use host::{paths, requests, settings};

/// SHA-256 of `data`, a story's and an order's cache key.
fn sha256(data: &[u8]) -> Vec<u8> {
    use sha2::Digest;
    sha2::Sha256::digest(data).to_vec()
}
use switchboard_plugin_api::sdk as util;

/// The diff as a plugin, which the workspace loads in its own process.
pub struct Diff;

/// What the diff says of itself: every setting of its own it reads, with
/// its default (the same as `switchboard`'s `assets/settings.json` lists
/// it with, which a test there keeps so).
pub fn manifest() -> Manifest {
    Manifest {
        protocol: PROTOCOL_VERSION,
        name: "diff".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        settings: vec![
            Setting::new("diff_order", true),
            Setting::new("diff_story", true),
            Setting::new("diff_story_pages", true),
            Setting::new("diff_story_progress", true),
            Setting::new("diff_story_notes", true),
            Setting::new("diff_story_ahead", true),
            Setting::new("diff_story_plan", false),
            Setting::new("diff_list_wide", true),
            Setting::new("diff_uncommitted", true),
            Setting::new("diff_ask", true),
            Setting::new("diff_ask_session", true),
            Setting::new("diff_beside_share", json!("75")),
        ],
    }
}

impl Plugin for Diff {
    fn manifest(&self) -> Manifest {
        manifest()
    }

    /// `<program> [--base <ref> | --working] [--session <id> --session-dir
    /// <dir>] [dir]`: the change in `dir` (where it runs, else) against
    /// `<ref>`, or, without one, its PR's base as the workspace says it
    /// (alone, the default branch, or the branch it was cut from when it tracks one), to the working tree with the
    /// diff_uncommitted setting on; with `--working`, the working tree's
    /// changes, staged and not; with `--session`, `ga` asks a copy of that
    /// Claude session (the tab's, which M-d passes), resumed from the
    /// directory `--session-dir` names.
    fn run(&self, program: &str, args: Vec<String>, host: Arc<dyn Host>) -> ExitCode {
        host::install(host);
        host::share_with_ui();
        let usage = || {
            eprintln!(
                "Usage: {program} [--base <ref> | --working] [--session <id> --session-dir <dir>] [dir]"
            );
            ExitCode::from(2)
        };
        let mut base = None;
        let mut working = false;
        let mut dir = None;
        let (mut session, mut session_dir) = (None, None);
        let mut rest = args.into_iter();
        while let Some(arg) = rest.next() {
            match arg.as_str() {
                "--base" => match rest.next() {
                    Some(b) if git::safe_ref(&b) => base = Some(b),
                    _ => return usage(),
                },
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
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| paths::home()));
        if working && base.is_some() {
            return usage();
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

    /// A tab's new commit: its story told and kept, with nothing drawn, so
    /// the diff opens on it. A story told with a warning says it on stderr.
    fn on(&self, event: Event, host: Arc<dyn Host>) -> Result<(), String> {
        host::install(host);
        match event {
            Event::Commit { dir, plan } => {
                if let Some(warning) = ahead::commit(&dir, &plan)? {
                    eprintln!("{warning}");
                }
                Ok(())
            }
        }
    }
}
