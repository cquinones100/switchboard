//! `switchboard claude-plugin-dirs`: what `CLAUDE_CODE_PLUGIN_DIRS` holds for
//! a Claude started now in one of Switchboard's panes, from the variable as
//! the shell has it (`setup::plugin::value_now`, the rule a new pane gets
//! it by): Switchboard's plugin added while Settings' `claude_plugin` is on,
//! taken out while it isn't, any other folder kept, and an empty line for
//! none. The `claude` function `wt`'s `init` prints runs it at each launch,
//! since a shell open from before the setting changed keeps the value it
//! started with, and a Claude typed there would otherwise miss Switchboard's
//! hooks and skills once setup moved them out of `~/.claude`, or run them
//! twice once it put them back. Ported from the `claude` function in
//! bin/worktree-from-ticket, which read tmux's global environment.

use std::process::ExitCode;

use crate::setup::{plugin, way};
use crate::{paths, settings};

pub fn main(args: Vec<String>) -> ExitCode {
    if !args.is_empty() {
        eprintln!("Usage: switchboard claude-plugin-dirs");
        return ExitCode::from(2);
    }
    let current = std::env::var(plugin::ENV).ok();
    let now = plugin::value_now(
        current.as_deref(),
        settings::is_on(way::SETTING),
        &paths::plugin_dir(),
        &paths::home(),
    );
    println!("{}", now.unwrap_or_default());
    ExitCode::SUCCESS
}
