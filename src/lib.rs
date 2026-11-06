//! Switchboard, as one library the `switchboard` binary runs and the tests
//! call into. Each part says at its top where it came from in the tmux and
//! Node version, so its history can be followed back. PORTING.md has the plan.
//!
//! Shared by every part:
//! - `paths`: where things are, through `~` and the clone.
//! - `util`: time, files, commands, JSON as JavaScript writes it.
//! - `settings`: the workspace's options (`assets/settings.json`).
//! - `requests`: the request audit for the Usage view.
//! - `platform`: what differs between macOS and Linux.
//! - `presence`: whether you're at the computer.
//! - `connector`: one call to a Claude connector.
//! - `worktree`: what the worktree commands share.
//! - `net`: HTTPS requests, each counted in the request audit.
//! - `integrations`: GitHub and Linear over their APIs.
//!
//! - `mux`: the multiplexer (in place of tmux).
//! - `agent`: what each Claude session is doing, from its hooks.
//! - `tabs`: what each tab is called and the color it wears.

pub mod agent;
pub mod cmd;
pub mod connector;
pub mod integrations;
pub mod mux;
pub mod net;
pub mod paths;
pub mod platform;
pub mod presence;
pub mod requests;
pub mod settings;
pub mod tabs;
pub mod util;
pub mod worktree;
