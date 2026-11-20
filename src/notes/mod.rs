//! Notes in the workspace: the server's side of it. The notes app itself
//! is a crate of its own, `switchboard-notes` (crates/notes), an app that
//! runs alone in any terminal and a plugin on Switchboard's plugin API
//! (`switchboard-plugin-api`), as the diff is: it never depends on this
//! crate, and reaches the workspace only through the API's `Host`. What is
//! here is what loads it and answers it, and Notes on the server, ported
//! from the scripts around it (tmux/notes-open.sh and the rest).
//!
//! - `host`: this program answering Notes for the workspace.
//! - `serve`: Notes on the server: M-N, M-e, a note's tab, a tab's note,
//!   the palette's commands, and `away`, what Notes' tab shows while it is
//!   beside another.
//!
//! The app's own modules are given at the paths they had (`notes::store`,
//! `notes::id`, `notes::render` and the rest), for the parts that read a
//! note themselves (wt, autopilot, `item-done`, the views' pictures).

use std::process::ExitCode;

pub mod host;
pub mod serve;
#[cfg(test)]
mod tests;

pub use switchboard_notes::{
    app, diagram, easy, editor, finder, fold, format, id, images, jumps, marks, menu, palette,
    plan, render, sidebar, socket, squash, store, sync, table, text,
};
use switchboard_plugin_api::Plugin;

/// Notes, as the plugin it is.
pub fn plugin() -> &'static dyn Plugin {
    &switchboard_notes::Notes
}

/// `switchboard notes`: the app run on this terminal, loaded as a plugin (a
/// build can't load one built for a newer protocol) with this program
/// answering for the workspace (`host`); `switchboard notes open <note>`,
/// `sidebar on|off` and `shown` ask the running one, as tmux/notes-open.sh
/// did; `away` is what the server runs in Notes' tab while it is beside
/// another (serve/away.rs).
pub fn main(args: Vec<String>) -> ExitCode {
    if let Err(why) = plugin().manifest().check() {
        eprintln!("switchboard notes: {why}");
        return ExitCode::FAILURE;
    }
    if args.first().is_some_and(|a| a == "away") {
        return serve::away::main();
    }
    plugin().run("switchboard notes", args, host::core())
}
