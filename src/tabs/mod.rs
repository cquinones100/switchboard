//! What a tab is called and what color it wears, as the tmux version's tab
//! bar showed them: tmux/tab-label.sh's label, tmux/accent-color.sh's color
//! and name, tmux/path-facts.sh's answer per directory, tmux/window-pr.sh's
//! PR prefix and tmux/ticket-title.sh's ticket title, put together for each
//! tab as tmux/window-names.sh did.
//!
//! The rule the scripts lived by holds here too: naming a tab runs on every
//! focus and tick, so nothing in it waits on the network or starts a
//! process. A directory's branch is read from git's own files, and a PR or
//! a ticket's title is asked for in the background and shown once known.
//!
//! - `color`: the color key, its hash and the color, and the directory's
//!   name; what the Claude statusline will call to keep the same color.
//! - `git`: where a directory sits in git, from git's files.
//! - `label`: the label's rules, with nothing in them that reads or asks.
//! - `names`: what is kept per directory and per lookup, and the lookups.
//! - `serve`: the server's side: naming the tabs, and what later parts read
//!   (`Server::tab_label`, `tab_color`, `tab_path`, `tab_pr`).

pub mod color;
pub mod git;
pub mod label;
pub mod names;
mod serve;

use std::collections::HashMap;

use crate::mux::TabId;

pub use color::{
    GRAY, Rgb, accent_color, accent_color_of, color_key, color_key_of, statusline_color,
};
pub use names::{Answer, Names};
pub use serve::Shown;

/// What the server keeps for the tab bar: each directory's answer and its
/// lookups, and what each tab shows.
#[derive(Default)]
pub struct TabNames {
    pub names: Names,
    pub shown: HashMap<TabId, Shown>,
}
