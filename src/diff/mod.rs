//! M-d's diff: a branch's changes as GitHub's "Files changed" shows them,
//! ported from diffview.nvim as M-d set it up (nvim/lua/plugins/diffview.lua,
//! nvim/lua/diffstory.lua, nvim/lua/diffask.lua).
//!
//! What Claude does for it comes first, each part tested without Claude:
//!
//! - `order`: the files in reading order, generated files last, and
//!   Haiku's order of the code by its paths.
//! - `story`: the diff told as a story: what Haiku reads, its answer read.
//! - `tell`: telling it, cached, and followed while another program tells it.
//! - `ask`: a question about the diff, answered by Haiku.

pub mod ask;
pub mod order;
pub mod story;
pub mod tell;
