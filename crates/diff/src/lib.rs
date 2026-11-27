//! The diff: a branch's changes as GitHub's "Files changed" shows them,
//! side by side in a terminal, told as a story. It is the first plugin on
//! Switchboard's plugin API.
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
pub mod clear;
pub mod git;
pub use switchboard_highlight as highlight;
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
/// settings view, alone, drawn from this, say the same. One only the
/// workspace has a use for (a pane beside) is left out of that view, and
/// the one that is the diff alone's is in no list of the workspace's.
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
                    "M-d's file list grouped into a few pages Claude names from the whole diff (Haiku, or Sonnet for a large change, as \"Model that tells the diff's story\" picks) and the commits' messages, each with a heading, two or three sentences that make the case for it to the colleagues reviewing it, and a line of what to watch for, read from the change's core to its edges, and told in chapters (]c and [c move between them, yS copies the story as Markdown); off, the files keep the reading order by kind. Sends the diff's text and the commit messages, not only the paths, to Claude",
                ),
            Setting::new("diff_story_progress", true)
                .said(
                    "Diff story progress",
                    "While M-d's story is told, its cover shows which model is telling it and what it is reading, each page as it names it and the time taken, t showing its answer itself as it comes; off, a spinner alone",
                ),
            Setting::new("diff_story_pages", true)
                .said(
                    "Diff story a page at a time",
                    "]p and [p in M-d's diff open the story's pages one at a time, each a diff of its own as if it were a commit, [p from the first going back to the whole diff; off, they do nothing",
                ),
            Setting::new("diff_story_notes", true)
                .said(
                    "Diff story notes on changes",
                    "M-d's diff shows the story's note over each change worth one, why it is needed, and names the page of a change that belongs to another one than its file; off, only the list's pages",
                ),
            Setting::new("diff_story_map", true)
                .said(
                    "Open the diff on what changes",
                    "M-d tells the diff's story in chapters of pages (the goal's chapter, one for each part it can be read without, and the side work) and opens on what changes: before and after in plain words and how and why for each page, an outline of the chapters and pages beside it. Enter reads down one column, each page's claim, what it decided and instead of what, and its changes, tests folded last; What runs shows the steps in the order they happen. The story's map, a box a page and an arrow to what each builds on, is a third view, and the files stay a view of their own (g s, g m and g f, or m), opening in the view used last, or on the talk while diff_story_talk is on; off, the story is one list of pages over the files. A story already told is told again the first time either way",
                ),
            Setting::new("diff_story_talk", true)
                .said(
                    "Open the diff as a talk",
                    "With the story in chapters (diff_story_map), M-d opens on the Talk view, first in the views' switch: the change in one sentence and one worked example taken from its own tests, then the flow that example takes, each step, and the code, a zoom at a time (Gist, Flow, Steps, Code; - and + zoom out and in, the left and right arrows go between slides), the example's value shown at every step. With no test giving one, it says so rather than make one up; off, the Talk view is gone and M-d opens as it did",
                ),
            // A choice of three, which the diff's own settings view, of
            // on/off rows, can't take: set in the workspace's Settings,
            // or by hand in the diff's own settings file alone.
            Setting::new("diff_story_model", "size")
                .said(
                    "Model that tells the diff's story",
                    "Which model tells M-d's story: by size, Sonnet for a change of more than 20 files or 1,500 changed lines and Haiku for any other; or always Haiku, or always Sonnet. A story that reads the tab's plan (diff_story_plan) is always told by Haiku, the plan going to Haiku alone. A story keeps the model that told it until the menu's Tell it again",
                )
                .only(Scope::Workspace),
            Setting::new("diff_story_ahead", true)
                .said(
                    "Diff story told ahead",
                    "When a tab's branch has a new commit, its M-d story is told in the background, so M-d opens on it at once rather than waiting under its cover; sends that branch's diff and commit messages to Claude (Haiku, or Sonnet for a large change, as \"Model that tells the diff's story\" picks) for each new commit, a run taking up to five and a half minutes",
                )
                .only(Scope::Workspace),
            Setting::new("diff_story_plan", false)
                .said(
                    "Diff story reads the tab's plan",
                    "M-d's story, told ahead or on opening, is given the tab's plan note (an autopilot item's plan) as well as the commits, so its pages can say what problem each solves and what was passed over; off, it reads the commits and the diff alone. Sends the plan note's words to Claude (Haiku, which tells every story that reads the plan)",
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
            Setting::new("diff_list_resize", true)
                .said(
                    "Resize the diff's file list",
                    "A drag of M-d's file list's right edge, lit under the mouse, or the pane resize keys (C-M-S-h and C-M-S-l) while the list has the keys, gives the list a width of its own, from 24 columns to half the diff's width, which every diff opens at after, E going between it and the longest path; the diff's menu (m, or a right click) has \"Use the file list's default width\". Off, the list takes the width it had before (30 columns, or its longest path with diff_list_wide and E), its edge takes no drag and the pane resize keys move the pane's divider",
                ),
            Setting::new("diff_beside_share", json!("75"))
                .said(
                    "Diff's share in the side panel",
                    "How much of the width beside the sidebar M-d's diff and lazygit take when they open in the side panel: half, two thirds or three quarters, the pane narrowed to the rest while one is open and given its own width back when it closes, or the side panel alone, the pane keeping its width. Where the side panel already gives them more, they take it. Dragging the line between them resizes for that one alone",
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

    /// `<program> [--base <ref> | --working] [--resize-key <key> <columns>]... [dir]`: the change in `dir` (where it runs, else) against
    /// `<ref>`, or, without one, its PR's base as the workspace says it
    /// (alone, the default branch, or the branch it was cut from when it tracks one), to the working tree with the
    /// diff_uncommitted setting on; with `--working`, the working tree's
    /// changes, staged and not; each `--resize-key` names a key
    /// bound to moving a pane's divider and the columns it moves it by (to
    /// the left when negative), which set the file list's width while the
    /// list has the keys (the workspace's default keys with none named);
    /// `--several` says the workspace's tab follows several PRs, which puts
    /// "Show another of this tab's PRs" in the menu.
    fn run(&self, program: &str, args: Vec<String>, host: Arc<dyn Host>) -> ExitCode {
        host::install(host);
        host::share_with_ui();
        let usage = || {
            eprintln!(
                "Usage: {program} [--base <ref> | --working] [--resize-key <key> <columns>]... [--several] [dir]"
            );
            ExitCode::from(2)
        };
        let mut base = None;
        let mut working = false;
        let mut dir = None;
        let mut resize_keys = Vec::new();
        let mut several = false;
        let mut rest = args.into_iter();
        while let Some(arg) = rest.next() {
            match arg.as_str() {
                "--base" => match rest.next() {
                    Some(b) if git::safe_ref(&b) => base = Some(b),
                    _ => return usage(),
                },
                // Passed by a server still running a build from before Ask
                // was removed, until it restarts on this one: taken and
                // passed over, so its M-d still opens the diff.
                "--session" | "--session-dir" => {
                    if rest.next().is_none() {
                        return usage();
                    }
                }
                // A key the diff can't take as a resize (Escape bound to
                // one) is passed over, the workspace keeping its binding.
                "--resize-key" => match (rest.next(), rest.next()) {
                    (Some(key), Some(columns)) => {
                        resize_keys.extend(app::ResizeKey::parse(&key, &columns));
                    }
                    _ => return usage(),
                },
                "--working" => working = true,
                "--several" => several = true,
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
        app::run(dir, base, working, resize_keys, several)
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
