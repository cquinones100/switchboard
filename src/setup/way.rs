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
//! - Claude Code's managed settings (an organization's policy, in the file
//!   `platform::claude_managed_settings` names) may set
//!   `disableSideloadFlags`, with which a Claude whose environment names a
//!   plugin directory refuses to start, so every pane's would. Setup keeps
//!   the old way there too, turning the setting off and saying why.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use serde_json::Value;

use super::{claude_md, claude_settings, plugin, skills};
use crate::{platform, settings, util};

/// The setting.
pub const SETTING: &str = "claude_plugin";

/// What a change of way leaves to the tabs already open: a pane's
/// environment names the plugin or not as it started (src/mux/server.rs),
/// and a shell left open keeps it, so a `claude` started again there would
/// have none of Switchboard after a move into the plugin, and every hook
/// twice after a move back. Said after every move.
pub const OPEN_TABS: &str = "a tab open now takes the change once it is reopened or the workspace restarts (switchboard kill-server, then switchboard)";

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

/// Whether Claude Code's managed settings in `file` set
/// `disableSideloadFlags`, with which Claude refuses to start when its
/// environment names a plugin directory (`plugin::ENV`). A file that is
/// missing or isn't JSON sets nothing.
pub fn sideload_disabled_in(file: &Path) -> bool {
    util::read_json(file)
        .and_then(|v| v.get("disableSideloadFlags").and_then(Value::as_bool))
        .unwrap_or(false)
}

/// Whether the computer's managed settings turn sideloading off.
pub fn sideload_disabled() -> bool {
    sideload_disabled_in(&platform::claude_managed_settings())
}

/// Why the plugin can't be loaded here, whatever the setting says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// Claude Code's managed settings set `disableSideloadFlags`.
    Policy,
    /// Claude Code older than `plugin::CLAUDE_VERSION`, as it says.
    TooOld((u64, u64, u64)),
}

/// What keeps the plugin from loading, if anything: the managed settings
/// first, a file read, then Claude Code's version, which runs it.
pub fn blocked() -> Option<Blocked> {
    if sideload_disabled() {
        return Some(Blocked::Policy);
    }
    too_old().map(Blocked::TooOld)
}

impl Blocked {
    /// Why Switchboard stays in `~/.claude`, with `off` saying where the
    /// setting is (" and the setting is off", or why it couldn't be turned
    /// off), and what to do once it no longer holds.
    pub fn why(&self, off: &str) -> String {
        match self {
            Blocked::Policy => format!(
                "Claude Code's managed settings ({}) set disableSideloadFlags, with which Claude won't start in a pane that names Switchboard's plugin, so its hooks, skills and instructions stay in ~/.claude{off}; if that policy is lifted, turn on Settings' \"Load Switchboard as a plugin\"",
                platform::claude_managed_settings().display()
            ),
            Blocked::TooOld(v) => format!(
                "Claude Code {} is older than {}, which loads Switchboard as a plugin, so its hooks, skills and instructions stay in ~/.claude{off}; once it is updated (claude update), turn on Settings' \"Load Switchboard as a plugin\"",
                words(*v),
                words(plugin::CLAUDE_VERSION)
            ),
        }
    }
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
        _ if plugin_on() && blocked().is_none() => Way::Plugin,
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
/// can't load the plugin here (`blocked`), when the setting is turned off
/// (but with `check`) and why is given to say.
pub fn settle(check: bool) -> (Way, Option<String>) {
    let way = current();
    let why = match blocked() {
        Some(blocked) if plugin_on() => Some(match choose(Way::Old, check) {
            Err(e) => {
                return (
                    Way::Old,
                    Some(blocked.why(&format!("; the setting couldn't be turned off: {e:#}"))),
                );
            }
            Ok(()) if check => blocked.why(""),
            Ok(()) => blocked.why(" and the setting is off"),
        }),
        _ => None,
    };
    set(way);
    (way, why)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sideloading_is_off_only_where_the_managed_settings_say_so() {
        let dir = std::env::temp_dir().join(format!("sb-way-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("managed-settings.json");
        assert!(!sideload_disabled_in(&file), "no file");
        for (text, off) in [
            (r#"{"disableSideloadFlags": true}"#, true),
            (r#"{"disableSideloadFlags": false}"#, false),
            (r#"{"disableSideloadFlags": "true"}"#, false),
            (r#"{"permissions": {"deny": []}}"#, false),
            ("not json", false),
        ] {
            std::fs::write(&file, text).unwrap();
            assert_eq!(sideload_disabled_in(&file), off, "{text}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn why_says_what_keeps_the_plugin_off_and_what_to_do_after() {
        let policy = Blocked::Policy.why(" and the setting is off");
        assert!(policy.contains("disableSideloadFlags"), "{policy}");
        assert!(
            policy.contains("stay in ~/.claude and the setting is off;"),
            "{policy}"
        );
        let old = Blocked::TooOld((2, 1, 100)).why("");
        assert!(
            old.starts_with("Claude Code 2.1.100 is older than 2.1.280"),
            "{old}"
        );
        assert!(old.contains("claude update"), "{old}");
    }
}
