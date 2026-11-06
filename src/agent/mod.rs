//! What each Claude session in the workspace is doing: Claude Code's hooks
//! run `switchboard hook <event>`, which hands the hook's JSON to the server,
//! and the server marks the pane working, waiting on you, or done, which its
//! tab shows as a badge in the status line. This is tmux/agent-state.sh on
//! the server's own model: pane options became `Agents`, a record per pane
//! the server holds, and the tmux calls became the server's own methods.
//!
//! - `state`: the record and its rules, with nothing in it that reads a
//!   file or starts a process.
//! - `sessions`: Claude Code's session files, which say which process is
//!   Claude, which session is headless and where a background job came from.
//! - `hook`: `switchboard hook`, the side Claude Code runs.
//! - `serve`: the server's side: hooks, focus, the keys, one Claude per tab.
//!
//! What it says goes in a message box (src/ui): nowhere for M-a to go, no
//! Claude for M-c, a session moved to a tab of its own; a first wait or
//! finish while you look elsewhere, and the first pin, bring their tips.
//! M-a walks the sidebar's Needs you (src/sidebar), which alone can tell a
//! tab that looks done is still working in the background. Left for the
//! parts that come later: the worktree tip (its keys come with the PR
//! views).

pub mod hook;
mod serve;
pub mod sessions;
mod state;

pub use serve::SEEN_DELAY;
pub use state::{Agents, Hook, PaneAgent, QUESTION_KINDS, State};
