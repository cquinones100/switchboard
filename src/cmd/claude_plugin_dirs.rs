//! `switchboard claude-plugin-dirs`: what `CLAUDE_CODE_PLUGIN_DIRS` holds for
//! a Claude started now in one of Switchboard's panes, from the variable as
//! the shell has it (`setup::plugin::value_now`, the rule a new pane gets
//! it by): Switchboard's plugin added unless Claude Code's managed settings
//! forbid it (src/setup/blocked.rs) or an earlier version's hooks are
//! still in `~/.claude/settings.json` (an older build's server still
//! running, which takes them out as the new one starts), where the plugin
//! would run each twice; any other folder kept, and an empty
//! line for none. The `claude` function `wt`'s `init` prints runs it at
//! each launch, since a shell open from before keeps the value it started
//! with. Ported from the `claude` function in
//! bin/worktree-from-ticket, which read tmux's global environment.

use std::process::ExitCode;

use crate::paths;
use crate::setup::{blocked, claude_settings, plugin};

pub fn main(args: Vec<String>) -> ExitCode {
    if !args.is_empty() {
        eprintln!("Usage: switchboard claude-plugin-dirs");
        return ExitCode::from(2);
    }
    let current = std::env::var(plugin::ENV).ok();
    let now = plugin::value_now(
        current.as_deref(),
        blocked::panes_load_plugin() && !claude_settings::has_hooks(),
        &paths::plugin_dir(),
        &paths::home(),
    );
    println!("{}", now.unwrap_or_default());
    ExitCode::SUCCESS
}
