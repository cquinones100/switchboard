//! What keeps the Claude in Switchboard's panes from loading its plugin
//! (src/setup/plugin.rs), which is the one way Switchboard reaches Claude
//! Code: its hooks, skills and instructions reach no other Claude of yours.
//!
//! - Claude Code's managed settings (an organization's policy, in the file
//!   `platform::claude_managed_settings` names) may set
//!   `disableSideloadFlags`, with which a Claude whose environment names a
//!   plugin directory refuses to start. There a pane names no plugin
//!   (`panes_load_plugin`), so its Claude starts, without Switchboard's
//!   parts, and setup says why.
//! - Claude Code older than 2.1.280 doesn't read the variable that names
//!   the plugin, so its panes' Claude has none of Switchboard either; setup
//!   says so, with `claude update`. `claude --version` is asked once a run,
//!   only here, never on the drawing path; a version that can't be read
//!   counts as new enough.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use serde_json::Value;

use super::plugin;
use crate::{platform, util};

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

/// Whether Claude Code's managed settings in `file` set
/// `disableSideloadFlags`, with which Claude refuses to start when its
/// environment names a plugin directory (`plugin::ENV`). A file that is
/// missing or isn't JSON sets nothing.
pub fn sideload_disabled_in(file: &Path) -> bool {
    util::read_json(file)
        .and_then(|v| v.get("disableSideloadFlags").and_then(Value::as_bool))
        .unwrap_or(false)
}

/// Whether a pane's environment names the plugin: everywhere but where the
/// computer's managed settings turn sideloading off, which would keep the
/// pane's Claude from starting at all. A small file read, when a pane or a
/// `claude` starts.
pub fn panes_load_plugin() -> bool {
    !sideload_disabled_in(&platform::claude_managed_settings())
}

/// Why the plugin can't be loaded here.
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
    if !panes_load_plugin() {
        return Some(Blocked::Policy);
    }
    claude_version()
        .filter(|v| *v < plugin::CLAUDE_VERSION)
        .map(Blocked::TooOld)
}

impl Blocked {
    /// Why the Claude in Switchboard's panes has none of it, and what to do.
    pub fn why(&self) -> String {
        match self {
            Blocked::Policy => format!(
                "Claude Code's managed settings ({}) set disableSideloadFlags, with which Claude won't start in a pane that names a plugin, so the Claude in Switchboard's panes starts without its plugin (tab states, M-a, its skills and instructions); a pane opened once that policy is lifted loads it",
                platform::claude_managed_settings().display()
            ),
            Blocked::TooOld(v) => format!(
                "Claude Code {} is older than {}, which loads Switchboard's plugin, so the Claude in its panes has none of it (tab states, M-a, its skills and instructions): claude update",
                words(*v),
                words(plugin::CLAUDE_VERSION)
            ),
        }
    }
}

/// A version as Claude Code writes it.
fn words(v: (u64, u64, u64)) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sideloading_is_off_only_where_the_managed_settings_say_so() {
        let dir = std::env::temp_dir().join(format!("sb-blocked-{}", std::process::id()));
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
        let policy = Blocked::Policy.why();
        assert!(policy.contains("disableSideloadFlags"), "{policy}");
        let old = Blocked::TooOld((2, 1, 100)).why();
        assert!(
            old.starts_with("Claude Code 2.1.100 is older than 2.1.280"),
            "{old}"
        );
        assert!(old.contains("claude update"), "{old}");
    }
}
