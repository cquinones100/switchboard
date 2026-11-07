//! Which way `switchboard setup` puts Switchboard's parts for Claude Code:
//! as its plugin, which only the Claude sessions in its panes load
//! (`Way::Plugin`, src/setup/plugin.rs), or in `~/.claude` as earlier
//! versions did, for every Claude of yours (`Way::Old`): the hooks in your
//! settings, the skills in `~/.claude/skills`, Switchboard's part imported
//! into your CLAUDE.md. Settings' "Load Switchboard as a plugin"
//! (`claude_plugin`) picks it, on by default; the parts follow it, and
//! changing it moves them over (`super::move_over`).
//!
//! The setting and what is in `~/.claude` have to agree: a pane names the
//! plugin while the setting is on, and Claude in it would run Switchboard's
//! hooks twice if they were in your settings too. So:
//!
//! - An install an earlier version made (Switchboard's hooks, skills or
//!   import in `~/.claude`) whose owner hasn't chosen is asked about once by
//!   the guided setup; a new build's first run moves nothing quietly and
//!   records the old way for it (`claude_plugin: false`).
//! - Claude Code older than 2.1.280 can't load the plugin, so setup keeps
//!   the old way with it whatever the setting says, and turns the setting
//!   off, saying why, rather than leave the panes naming a plugin that a
//!   later Claude Code would load on top of the hooks in `~/.claude`.
//!   `claude --version` is asked once a run, only here, never on the
//!   drawing path; a version that can't be read leaves it to the setting.

use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use serde_json::Value;

use super::{claude_md, claude_settings, plugin, skills};
use crate::{settings, util};

/// The setting.
pub const SETTING: &str = "claude_plugin";

/// How Switchboard's parts reach Claude Code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    /// Switchboard's plugin, loaded by the Claude in its panes.
    Plugin,
    /// `~/.claude`, as earlier versions put them.
    Old,
}

/// The way this run settled on: 0 for none yet.
static SETTLED: AtomicU8 = AtomicU8::new(0);

/// Claude Code's version, as `claude --version` says it, once a run; `None`
/// when it isn't installed or says something else.
pub fn claude_version() -> Option<(u64, u64, u64)> {
    static VERSION: OnceLock<Option<(u64, u64, u64)>> = OnceLock::new();
    *VERSION.get_or_init(|| {
        let claude = util::which("claude")?;
        let out = Command::new(claude)
            .arg("--version")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        plugin::claude_version(&String::from_utf8_lossy(&out.stdout))
    })
}

/// Claude Code's version when it is known to be too old for the plugin.
pub fn too_old() -> Option<(u64, u64, u64)> {
    claude_version().filter(|v| *v < plugin::CLAUDE_VERSION)
}

/// A version as Claude Code writes it.
pub fn words(v: (u64, u64, u64)) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

/// Whether the setting is on, read from the file now.
pub fn plugin_on() -> bool {
    settings::setting(SETTING) != Some(Value::Bool(false))
}

/// Whether you have chosen, the setting written in your file.
pub fn chosen() -> bool {
    settings::is_set(SETTING)
}

/// Whether an earlier version's parts are in `~/.claude`: Switchboard's
/// hooks in your settings, its skills, or the import of its part.
pub fn old_install() -> bool {
    claude_settings::has_hooks() || skills::installed() || claude_md::imports_part()
}

/// The way for this run: the one it settled on, else what the setting and
/// Claude Code's version say, writing nothing.
pub fn current() -> Way {
    match SETTLED.load(Ordering::Relaxed) {
        1 => Way::Plugin,
        2 => Way::Old,
        _ if plugin_on() && too_old().is_none() => Way::Plugin,
        _ => Way::Old,
    }
}

/// This run's way from now on.
pub fn set(way: Way) {
    SETTLED.store(
        match way {
            Way::Plugin => 1,
            Way::Old => 2,
        },
        Ordering::Relaxed,
    );
}

/// The way chosen, written to the setting (unless `check`, which writes
/// nothing) and this run's from now on.
pub fn choose(way: Way, check: bool) -> anyhow::Result<()> {
    set(way);
    if check {
        return Ok(());
    }
    settings::write_setting(SETTING, Value::Bool(way == Way::Plugin))?;
    settings::reload_settings();
    Ok(())
}

/// The way for a run that changes things: the setting's, unless Claude Code
/// is too old for the plugin, when the setting is turned off (but with
/// `check`) and why is given to say.
pub fn settle(check: bool) -> (Way, Option<String>) {
    let way = current();
    let why = match too_old() {
        Some(v) if plugin_on() => {
            if let Err(e) = choose(Way::Old, check) {
                return (
                    Way::Old,
                    Some(format!(
                        "Claude Code {} is older than {}, which loads Switchboard as a plugin, so its hooks, skills and instructions stay in ~/.claude; the setting couldn't be turned off: {e:#}",
                        words(v),
                        words(plugin::CLAUDE_VERSION)
                    )),
                );
            }
            Some(format!(
                "Claude Code {} is older than {}, which loads Switchboard as a plugin, so its hooks, skills and instructions stay in ~/.claude{}; once it is updated (claude update), turn on Settings' \"Load Switchboard as a plugin\"",
                words(v),
                words(plugin::CLAUDE_VERSION),
                if check { "" } else { " and the setting is off" }
            ))
        }
        _ => None,
    };
    set(way);
    (way, why)
}
