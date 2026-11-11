// The integration tests, one binary: each file a module, so cargo links one
// test binary rather than one per file. Run one file's tests with
// `cargo test --test it <name>::`. The tests that change the process's
// environment (tests/day.rs, tests/integrations.rs, tests/update.rs) are
// binaries of their own, since `set_var` here would race every other test.

#[path = "../common/mod.rs"]
mod common;

mod agent;
mod apply_actions;
mod autopilot;
mod awake;
mod batch_tabs;
mod claude_here;
mod claude_reply;
mod codex_review;
mod demo;
mod diff;
mod headless;
mod land_lock;
mod leak_check;
mod links;
mod mission;
mod mux;
mod notes_workspace;
mod persist;
mod pr_risk;
mod setup;
mod sidebar;
mod skill_path;
mod slack_relay;
mod sources;
mod stand_ins;
mod status;
mod tabs;
mod try_;
mod ui;
mod update_main;
mod upgrade;
mod usage;
mod ux2;
mod views;
mod wait_ci;
mod wait_deploy;
mod watch_note;
mod work_keys;
mod worktrees;
mod wt_jump;
mod wt_revive;
