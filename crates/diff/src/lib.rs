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
//! - `update`: alone, its look for a newer release and `switchboard-diff
//!   update`.
//! - `host`: who answers for the workspace.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use serde_json::json;
use switchboard_plugin_api::{Event, Host, Manifest, PROTOCOL_VERSION, Plugin, Scope, Setting};

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
pub mod update;

use host::{paths, requests, settings};

/// SHA-256 of `data`, a story's and an order's cache key.
fn sha256(data: &[u8]) -> Vec<u8> {
    use sha2::Digest;
    sha2::Sha256::digest(data).to_vec()
}
use switchboard_plugin_api::sdk as util;

/// The diff as a plugin, which the workspace loads in its own process.
pub struct Diff;

/// The setting that has the diff alone look for a newer release of itself.
pub const UPDATE_CHECK: &str = "diff_update_check";

/// What the diff says of itself: every setting of its own it reads, with
/// its default, its name and what it does (the same as `switchboard`'s
/// `assets/settings.json` lists it with, which a test there keeps so), so
/// the workspace's Settings, drawn from that file, and the diff's own
/// settings view, alone, drawn from this, say the same. One only the workspace has a use for (a tab's session,
/// a pane beside) is left out of that view, and the one that is the diff
/// alone's is in no list of the workspace's.
pub fn manifest() -> Manifest {
    Manifest {
        protocol: PROTOCOL_VERSION,
        name: "diff".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        settings: vec![
            Setting::new("diff_order", true)
                .said(
                    "Files in reading order",
                    "M-d lists the changed files flat, documentation first and tests last, with Haiku ordering the code from most abstract to most concrete; off, flat, A to Z",
                ),
            Setting::new("diff_story", true)
                .said(
                    "Diff told as a story",
                    "M-d's file list grouped into a few pages Haiku names from the whole diff and the commits' messages, each with a heading, two or three sentences that make the case for it to the colleagues reviewing it, and a line of what to watch for, read from the change's core to its edges, and told as a map, in chapters (]c and [c move between them, yS copies the story as Markdown); off, the files keep the reading order by kind. Sends the diff's text and the commit messages, not only the paths, to Claude",
                ),
            Setting::new("diff_story_progress", true)
                .said(
                    "Diff story progress",
                    "While M-d's story is told, its cover shows what Haiku is reading, each page as Haiku names it and the time taken, t showing Haiku's answer itself as it comes; off, a spinner alone",
                ),
            Setting::new("diff_story_pages", true)
                .said(
                    "Diff story a page at a time",
                    "]p and [p in M-d's diff open the story's pages one at a time, each a diff of its own as if it were a commit, [p from the first going back to the whole diff; off, they do nothing",
                ),
            Setting::new("diff_story_notes", true)
                .said(
                    "Diff story notes on changes",
                    "M-d's diff shows Haiku's note over each change worth one, why it is needed, and names the page of a change that belongs to another one than its file; off, only the list's pages",
                ),
            Setting::new("diff_story_map", true)
                .said(
                    "Tell the diff's story as a map",
                    "M-d tells the diff's story as a map: one story in chapters of pages (the goal's chapter, one for each part it can be read without, and the side work), each page with the pages it builds on, as Haiku tells it, and a change shown on every page that can't be read without it. The diff gets a Map mode beside its files (g m and g f, or m), the chapters in a strip across its top, opening in the mode used last; off, the story is one list of pages and there is no Map mode. A story already told is told again the first time either way",
                ),
            Setting::new("diff_story_ahead", true)
                .said(
                    "Diff story told ahead",
                    "When a tab's branch has a new commit, its M-d story is told in the background, so M-d opens on it at once rather than waiting under its cover; sends that branch's diff and commit messages to Claude (Haiku) for each new commit, a run taking up to five and a half minutes",
                )
                .only(Scope::Workspace),
            Setting::new("diff_story_plan", false)
                .said(
                    "Diff story reads the tab's plan",
                    "M-d's story, told ahead or on opening, is given the tab's plan note (an autopilot item's plan) as well as the commits, so its pages can say what problem each solves and what was passed over; off, it reads the commits and the diff alone. Sends the plan note's words to Claude (Haiku)",
                )
                .only(Scope::Workspace),
            Setting::new("diff_uncommitted", true)
                .said(
                    "Uncommitted changes in M-d",
                    "M-d's diff runs from the PR's base to the working tree, marking files with changes not yet committed and listing them in a last page; off, it shows the branch's commits only",
                ),
            Setting::new("diff_list_wide", true)
                .said(
                    "Diff file list wide",
                    "M-d's file list opens wide enough for its longest path (at most half the screen), E narrowing it; off, it opens narrow and E widens it",
                ),
            Setting::new("diff_ask", true)
                .said(
                    "Ask Claude about the diff",
                    "ga in M-d's diff, and M-p's command while the diff is open, ask Haiku a question about the change: it reads the story, the diff and where the cursor is, and may read the repository's files to answer, its answer streamed into a window beside the diff. Sends the diff and the question to Claude",
                ),
            Setting::new("diff_ask_session", true)
                .said(
                    "Ask in the tab's session",
                    "With Ask Claude about the diff on, ga asks a copy of the tab's own Claude session, which knows the conversation that made the change, on that session's model; the tab's conversation is left as it was, and Haiku answers in its place when the session can't be resumed. Off, it asks a fresh Haiku with the story and the diff alone, which costs less",
                )
                .only(Scope::Workspace),
            Setting::new("diff_beside_share", json!("75"))
                .said(
                    "Diff's share beside a narrow pane",
                    "How much of the width beside the sidebar M-d's diff takes when it opens beside a narrow lone pane: half, two thirds or three quarters, the pane narrowed to the rest while the diff is open and given its own width back when it closes, or the spare columns alone, the pane keeping its width. Where the spare columns already give the diff more, it takes them. Dragging the line between them resizes for that diff alone",
                )
                .only(Scope::Workspace),
            Setting::new(UPDATE_CHECK, true)
                .said(
                    "Check for a newer switchboard-diff",
                    "Once a day, switchboard-diff run alone asks GitHub for its newest release and says when one is out; nothing of yours is sent. Off, nothing is asked, as with Switchboard's own update check off",
                )
                .only(Scope::Alone),
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
